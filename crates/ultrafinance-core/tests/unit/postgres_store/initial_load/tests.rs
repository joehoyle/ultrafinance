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
        let rows = client.query("SELECT s.external_id,m.name,m.website,m.logo_url,m.logo_source,to_json(m.markets)::text,to_json(m.aliases)::text,to_json(m.sources)::text,l.name,l.address,l.country FROM source_identities s JOIN merchants m ON m.id=s.merchant_id LEFT JOIN location_records l ON (l.source,l.external_id)=(s.source,s.external_id) ORDER BY s.external_id", &[])?;
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
        let count: i64 = client.query_one("SELECT count(*) FROM location_records l JOIN source_identities s ON (s.source,s.external_id,s.merchant_id)=(l.source,l.external_id,merchant_public_id(l.merchant_id))", &[])?.get(0);
        assert_eq!(count, 7);
        let words: i64 = client.query_one("SELECT count(*) FROM merchant_search_words", &[])?.get(0);
        assert!(words > 0);
        let missing: i64 = client.query_one("SELECT count(*) FROM (SELECT DISTINCT unnest(tsvector_to_array(tokens)) AS word FROM merchant_search) expected WHERE char_length(word)>=3 AND NOT EXISTS(SELECT 1 FROM merchant_search_words w WHERE w.word=expected.word)", &[])?.get(0);
        assert_eq!(missing, 0);
        let enabled: i64 = client.query_one("SELECT count(*) FROM pg_trigger WHERE tgrelid='merchant_search'::regclass AND tgname IN ('merchant_search_words_insert','merchant_search_words_update') AND tgenabled='O'", &[])?.get(0);
        assert_eq!(enabled, 2);
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
    let mut repeated_source = duplicate[0].clone();
    // Different rule keys sort this duplicate into a later batch. The retained
    // source primary key must reject it even without the import_seen table.
    repeated_source.merchant.name = "Zulu Duplicate".into();
    duplicate.push(repeated_source);
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
        client.batch_execute("CREATE FUNCTION break_initial_ownership() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.merchant_id=-1; RETURN NEW; END $$; CREATE TRIGGER break_ownership BEFORE INSERT ON location_records FOR EACH ROW EXECUTE FUNCTION break_initial_ownership()")?;
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
        client.batch_execute("DROP TRIGGER break_ownership ON location_records; DROP FUNCTION break_initial_ownership()")?;
        Ok(())
    })?;
    load(&store, records()?, false)?;
    Ok(())
}

#[test]
fn initial_load_restores_vocabulary_triggers_after_failure_and_commit() -> Result<()> {
    let store = MerchantStore::temporary()?;
    store.0.run(|client| {
        client.batch_execute("ALTER TABLE merchant_search ENABLE ALWAYS TRIGGER merchant_search_words_insert; ALTER TABLE merchant_search DISABLE TRIGGER merchant_search_words_update")?;
        Ok(())
    })?;
    let modes = |store: &MerchantStore| -> Result<Vec<String>> {
        store.0.run(|client| Ok(client.query("SELECT tgenabled::text FROM pg_trigger WHERE tgrelid='merchant_search'::regclass AND tgname IN ('merchant_search_words_insert','merchant_search_words_update') ORDER BY tgname", &[])?.iter().map(|r| r.get(0)).collect()))
    };
    let before = modes(&store)?;
    let mut duplicate = records()?;
    duplicate.push(duplicate[0].clone());
    assert!(load(&store, duplicate, false).is_err());
    assert_eq!(modes(&store)?, before);
    load(&store, records()?, true)?;
    assert_eq!(modes(&store)?, before);
    load(&store, records()?, false)?;
    assert_eq!(modes(&store)?, before);
    store.0.run(|client| {
        // The restored trigger must still maintain newly inserted vocabulary.
        client.batch_execute("INSERT INTO merchants(id,name) VALUES ('word-test','Word test'); INSERT INTO merchant_search(merchant_id,text) VALUES (merchant_key('word-test'),'uniquepostloadword')")?;
        assert!(client.query_one("SELECT EXISTS(SELECT 1 FROM merchant_search_words WHERE word='uniquepostloadword')", &[])?.get::<_, bool>(0));
        Ok(())
    })?;
    Ok(())
}

#[test]
fn initial_load_rejects_wrong_but_existing_location_owner_and_restores_trigger() -> Result<()> {
    let store = MerchantStore::temporary()?;
    store.0.run(|client| {
        client.batch_execute("ALTER TABLE location_records ENABLE ALWAYS TRIGGER location_merchant; CREATE FUNCTION corrupt_initial_location_owner() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.merchant_id=COALESCE((SELECT merchant_key FROM merchants WHERE merchant_key<>NEW.merchant_id LIMIT 1),NEW.merchant_id); RETURN NEW; END $$; CREATE TRIGGER corrupt_initial_location_owner BEFORE INSERT ON location_records FOR EACH ROW EXECUTE FUNCTION corrupt_initial_location_owner()")?;
        Ok(())
    })?;
    let error = load(&store, records()?, false).unwrap_err();
    assert!(
        format!("{error:#}").contains("ownership does not match"),
        "{error:#}"
    );
    assert_eq!(store.stats()?.total, 0);
    store.0.run(|client| {
        assert_eq!(client.query_one("SELECT tgenabled::text FROM pg_trigger WHERE tgrelid='location_records'::regclass AND tgname='location_merchant'", &[])?.get::<_, String>(0), "A");
        client.batch_execute("DROP TRIGGER corrupt_initial_location_owner ON location_records; DROP FUNCTION corrupt_initial_location_owner()")?;
        Ok(())
    })?;
    load(&store, records()?, false)?;
    store.0.run(|client| {
        assert_eq!(client.query_one("SELECT tgenabled::text FROM pg_trigger WHERE tgrelid='location_records'::regclass AND tgname='location_merchant'", &[])?.get::<_, String>(0), "A");
        Ok(())
    })?;
    Ok(())
}

#[test]
fn grouped_initial_load_is_independent_of_fetch_boundaries() -> Result<()> {
    let mut csv = csv::Writer::from_writer(Vec::new());
    csv.write_record([
        "fsq_place_id",
        "name",
        "country",
        "address",
        "website",
        "fsq_category_ids",
        "date_closed",
    ])?;
    let names = [
        "Café Group",
        "Café Group LLC",
        "No Website",
        "Shared Page",
        "Tiny",
        "Invalid",
        "Café Group",
    ];
    let sites = [
        "https://cafe.test/a",
        "https://www.cafe.test/b",
        "",
        "https://facebook.com/shared",
        "https://tiny.test",
        "not-a-url",
        "https://cafe.test/c",
    ];
    for i in 0..140 {
        csv.write_record([
            i.to_string(),
            names[i % 7].into(),
            if i % 2 == 0 { "CA" } else { "US" }.into(),
            format!("{i} Main Street"),
            sites[i % 7].into(),
            "[\"shop\"]".into(),
            "".into(),
        ])?;
    }
    let input = String::from_utf8(csv.into_inner()?)?;
    let mut inputs = crate::foursquare::prepare(&input, "global")?.0;
    assert_eq!(inputs.len(), 140);
    inputs.sort_by(|a, b| a.external_id.cmp(&b.external_id));
    for (i, r) in inputs.iter_mut().enumerate() {
        if i % 11 == 0 {
            r.merchant.logo_url = Some(format!("https://logo.test/{i}.png"));
            r.merchant.logo_source = Some("fixture".into());
        }
    }
    let regular = MerchantStore::temporary()?;
    for chunk in inputs.chunks(7) {
        regular.reconcile_import(chunk.to_vec(), false, 7)?;
    }
    let expected = evidence(&regular)?;
    let derived = |store: &MerchantStore| -> Result<Vec<String>> {
        store.0.run(|client| Ok(client.query("SELECT s.external_id||'|'||q.text||'|'||k.normalized_name||'|'||COALESCE(k.rule_name,'')||'|'||COALESCE(k.rule_host,'')||'|'||(SELECT string_agg(a.normalized,',' ORDER BY a.normalized) FROM aliases a WHERE a.merchant_id=merchant_key(s.merchant_id)) FROM source_identities s JOIN merchant_search q ON q.merchant_id=merchant_key(s.merchant_id) JOIN merchant_identity_keys k ON k.merchant_id=merchant_key(s.merchant_id) ORDER BY s.external_id", &[])?.iter().map(|r|r.get(0)).collect()))
    };
    let expected_derived = derived(&regular)?;
    for size in [1, 7, 32] {
        let store = MerchantStore::temporary()?;
        let records = inputs.clone();
        let report = store
            .initial_foursquare_load(records.len(), false, size, move |consume| consume(records))?;
        assert_eq!(report.delta.added, inputs.len());
        assert_eq!(evidence(&store)?, expected, "fetch size {size}");
        assert_eq!(derived(&store)?, expected_derived, "fetch size {size}");
        assert_eq!(
            report.dedupe.candidates,
            inputs.len() - store.stats()?.total
        );
    }
    Ok(())
}
