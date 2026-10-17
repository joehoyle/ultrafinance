use super::*;
use crate::store::MerchantStore;
use serde_json::{Value, json};

fn records() -> Result<Vec<SourceRecord>> {
    let csv = "fsq_place_id,name,country,address,website,fsq_category_ids,date_closed\n\
        1,Maple Cafe,CA,1 Main St,https://maple.example,[\"restaurant\"],\n\
        2,Other Shop,US,2 Main St,,[\"shop\"],\n\
        3,Maple Cafe Ltd,US,3 Main St,https://maple.example,[\"restaurant\"],\n\
        4,Maple Cafe,CA,4 Main St,https://maple.example,[\"restaurant\"],\n\
        5,Other Shop,CA,5 Main St,,[\"shop\"],\n\
        6,Shared Page,CA,6 Main St,https://facebook.com/shared,[\"shop\"],\n\
        7,Shared Page,CA,7 Main St,https://facebook.com/shared,[\"shop\"],\n";
    // Use the actual adapter, including its address and country policy.
    let mut records = crate::foursquare::prepare(csv, "global")?.0;
    assert_eq!(records.len(), 7);
    records[2].merchant.logo_url = Some("https://maple.example/logo.png".into());
    records[2].merchant.logo_source = Some("fixture".into());
    Ok(records)
}
fn load(
    store: &MerchantStore,
    records: Vec<SourceRecord>,
    dry: bool,
) -> Result<crate::dedupe::ImportReport> {
    store.initial_foursquare_load(records.len(), dry, 2, move |consume| {
        for chunk in records.chunks(2) {
            consume(chunk.to_vec())?;
        }
        Ok(())
    })
}
fn indexes(store: &MerchantStore) -> Result<Vec<String>> {
    store.0.run(|client| Ok(client.query("SELECT indexname || ':' || indexdef FROM pg_indexes WHERE schemaname=current_schema() ORDER BY indexname", &[])?.iter().map(|r| r.get(0)).collect()))
}
fn evidence(store: &MerchantStore) -> Result<Value> {
    store.0.run(|client| {
        let rows = client.query("SELECT s.external_id,m.name,m.website,m.logo_url,m.logo_source,to_json(m.markets)::text,to_json(m.aliases)::text,to_json(m.sources)::text,l.name,l.address,l.country FROM source_records s JOIN merchants m ON m.id=s.merchant_id LEFT JOIN location_records l ON (l.merchant_source,l.merchant_external_id)=(s.source,s.external_id) ORDER BY s.external_id", &[])?;
        Ok(json!(rows.iter().map(|r| (0..11).map(|i| r.get::<_, Option<String>>(i)).collect::<Vec<_>>()).collect::<Vec<_>>()))
    })
}

#[test]
fn initial_load_matches_incremental_evidence_and_supports_refresh() -> Result<()> {
    let initial = MerchantStore::temporary()?;
    let regular = MerchantStore::temporary()?;
    let expected_indexes = indexes(&initial)?;
    let records = records()?;
    for chunk in records.chunks(2) {
        regular.reconcile_import(chunk.to_vec(), false, 2)?;
    }
    let result = load(&initial, records.clone(), false)?;
    assert_eq!(result.delta.added, records.len());
    assert_eq!(initial.stats()?.total, regular.stats()?.total);
    assert_eq!(evidence(&initial)?, evidence(&regular)?);
    assert_eq!(indexes(&initial)?, expected_indexes);
    // Every usable outlet is owned by the same source identity after publication.
    initial.0.run(|client| {
        let count: i64 = client.query_one("SELECT count(*) FROM location_records l JOIN source_records s ON (s.source,s.external_id)=(l.merchant_source,l.merchant_external_id)", &[])?.get(0);
        assert_eq!(count, 7);
        let words: i64 = client.query_one("SELECT count(*) FROM merchant_search_words", &[])?.get(0);
        assert!(words > 0);
        Ok(())
    })?;
    let refresh = initial.reconcile_import(records, false, 2)?;
    assert_eq!(refresh.delta.unchanged, 7);
    assert_eq!(evidence(&initial)?, evidence(&regular)?);
    Ok(())
}

#[test]
fn initial_load_rolls_back_preview_duplicates_and_producer_failure() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let original = indexes(&store)?;
    load(&store, records()?, true)?;
    assert_eq!(store.stats()?.total, 0);
    assert_eq!(indexes(&store)?, original);
    let mut duplicate = records()?;
    duplicate.push(duplicate[0].clone());
    assert!(load(&store, duplicate, false).is_err());
    assert_eq!(store.stats()?.total, 0);
    assert_eq!(indexes(&store)?, original);
    let failure = store.initial_foursquare_load(7, false, 2, |consume| {
        consume(records()?)?;
        bail!("injected producer failure")
    });
    assert!(failure.is_err());
    assert_eq!(store.stats()?.total, 0);
    assert_eq!(indexes(&store)?, original);
    // A failed load can restart without schema repair or cleanup.
    load(&store, records()?, false)?;
    assert!(
        load(&store, records()?, false)
            .unwrap_err()
            .to_string()
            .contains("empty catalog")
    );
    assert_eq!(indexes(&store)?, original);
    Ok(())
}

#[test]
fn initial_load_validates_foreign_keys_before_publishing() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let before = indexes(&store)?;
    store.0.run(|client| {
        client.batch_execute("CREATE FUNCTION break_initial_ownership() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.merchant_id='missing'; RETURN NEW; END $$; CREATE TRIGGER break_ownership BEFORE INSERT ON source_records FOR EACH ROW EXECUTE FUNCTION break_initial_ownership()")?;
        Ok(())
    })?;
    let error = load(&store, vec![records()?.remove(0)], false).unwrap_err();
    assert!(
        format!("{error:#}").contains("foreign key constraint"),
        "{error:#}"
    );
    assert_eq!(store.stats()?.total, 0);
    assert_eq!(indexes(&store)?, before);
    store.0.run(|client| {
        client.batch_execute("DROP TRIGGER break_ownership ON source_records; DROP FUNCTION break_initial_ownership()")?;
        Ok(())
    })?;
    load(&store, records()?, false)?;
    Ok(())
}
