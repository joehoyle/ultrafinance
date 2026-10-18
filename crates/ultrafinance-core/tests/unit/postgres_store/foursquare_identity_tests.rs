use super::*;
use crate::store::MerchantStore;

fn records() -> Result<Vec<SourceRecord>> {
    Ok(crate::foursquare::prepare("fsq_place_id,name,country,address,website,fsq_category_ids,date_closed\n1,Cafe,CA,1 Main St,https://cafe.test,[\"shop\"],\n2,Cafe,CA,,https://cafe.test,[\"shop\"],\n", "ca")?.0)
}

#[test]
fn compact_identity_replays_and_addressless_transition_keep_ids() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let inputs = records()?;
    let first = store.reconcile_import(inputs.clone(), false, 1)?;
    assert_eq!(first.delta.added, 2);
    let id = store.resolve_source("foursquare", "place:1")?.unwrap();
    let outlet = store.locations(&id)?.remove(0);
    assert_eq!(
        store.resolve_source("foursquare", "place:2")?,
        Some(id.clone())
    );
    store.0.run(|c| {
        assert_eq!(
            c.query_one(
                "SELECT count(*) FROM source_records WHERE source='foursquare'",
                &[]
            )?
            .get::<_, i64>(0),
            0
        );
        assert_eq!(
            c.query_one("SELECT count(*) FROM foursquare_unlocated", &[])?
                .get::<_, i64>(0),
            1
        );
        assert_eq!(
            c.query_one(
                "SELECT count(*) FROM location_records WHERE merchant_source IS NOT NULL",
                &[]
            )?
            .get::<_, i64>(0),
            0
        );
        Ok(())
    })?;
    let replay = store.reconcile_import(inputs.clone(), false, 1)?;
    assert_eq!(replay.delta.unchanged, 2);
    assert_eq!(store.locations(&id)?[0].location.id, outlet.location.id);
    let mut addressed = inputs[1].clone();
    addressed.raw["places"][0]["address"] = serde_json::json!("2 Main St");
    store.reconcile_import(vec![addressed], false, 1)?;
    assert_eq!(
        store.resolve_source("foursquare", "place:2")?,
        Some(id.clone())
    );
    assert_eq!(store.locations(&id)?.len(), 2);
    store.0.run(|c| {
        assert_eq!(
            c.query_one("SELECT count(*) FROM foursquare_unlocated", &[])?
                .get::<_, i64>(0),
            0
        );
        assert_eq!(
            c.query_one(
                "SELECT count(*) FROM source_identities WHERE source='foursquare'",
                &[]
            )?
            .get::<_, i64>(0),
            2
        );
        Ok(())
    })?;
    Ok(())
}

#[test]
fn migration_removes_foursquare_evidence_and_preserves_import_identity() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let inputs = records()?;
    store.reconcile_import(inputs.clone(), false, 5000)?;
    let merchant = store.resolve_source("foursquare", "place:1")?.unwrap();
    let outlet = store.locations(&merchant)?[0].location.id.clone();
    let owner = merchant.clone();
    store.0.run(move |c| {
        let mut tx=c.transaction()?;
        tx.batch_execute(include_str!("version_18_public_ids.sql"))?;
        tx.batch_execute("DROP VIEW source_identities; DROP TABLE foursquare_unlocated; ALTER TABLE source_records DROP CONSTRAINT source_records_no_foursquare; UPDATE ultrafinance_schema SET version=17")?;
        for record in inputs {
            crate::columns::postgres_source_records(&mut tx,&record.source,&record.external_id,&owner,&serde_json::to_string(&record)?)?;
        }
        tx.batch_execute("UPDATE location_records SET merchant_source='foursquare',merchant_external_id=external_id WHERE source='foursquare'")?;
        tx.commit()?;
        Ok(())
    })?;
    let migrated = PostgresStore::connect(store.temporary_url(), true)?;
    assert_eq!(
        migrated.resolve_source("foursquare", "place:1")?,
        Some(merchant.clone())
    );
    assert_eq!(
        migrated.resolve_source("foursquare", "place:2")?,
        Some(merchant.clone())
    );
    assert_eq!(migrated.locations(&merchant)?[0].location.id, outlet);
    migrated.run(|c| {
        assert_eq!(
            c.query_one("SELECT count(*) FROM source_records", &[])?
                .get::<_, i64>(0),
            0
        );
        assert_eq!(
            c.query_one("SELECT count(*) FROM foursquare_unlocated", &[])?
                .get::<_, i64>(0),
            1
        );
        assert_eq!(
            c.query_one("SELECT version FROM ultrafinance_schema", &[])?
                .get::<_, i32>(0),
            crate::store::POSTGRES_SCHEMA_VERSION
        );
        Ok(())
    })?;
    Ok(())
}

#[test]
fn addressless_links_manual_corrections_and_preview_survive_reimport() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let inputs = records()?;
    store.reconcile_import(inputs.clone(), false, 5000)?;
    let mut target = inputs[0].merchant.clone();
    target.id = "reviewed-target".into();
    target.name = "Reviewed Cafe".into();
    store.put(&target)?;
    store.link("foursquare", "place:2", &target.id)?;
    let mut addressed = inputs[1].clone();
    addressed.raw["places"][0]["address"] = serde_json::json!("New Street");
    store.reconcile_import(vec![addressed.clone()], true, 5000)?;
    assert!(store.locations(&target.id)?.is_empty());
    assert_eq!(
        store.resolve_source("foursquare", "place:2")?,
        Some(target.id.clone())
    );
    store.reconcile_import(vec![addressed.clone()], false, 5000)?;
    assert_eq!(store.get(&target.id)?.unwrap().name, "Reviewed Cafe");
    assert_eq!(store.locations(&target.id)?.len(), 1);
    assert_eq!(
        store
            .reconcile_import(vec![addressed], false, 5000)?
            .delta
            .unchanged,
        1
    );
    let empty = MerchantStore::temporary()?;
    let duplicate = inputs[1].clone();
    assert!(
        empty
            .initial_foursquare_load(2, false, 1, move |consume| {
                consume(vec![duplicate.clone()])?;
                consume(vec![duplicate])
            })
            .is_err()
    );
    assert_eq!(empty.stats()?.total, 0);
    Ok(())
}

#[test]
fn incremental_duplicate_reports_stay_bounded_without_losing_counts() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let template = records()?.remove(1);
    let inputs: Vec<_> = (0..6002)
        .map(|i| {
            let mut r = template.clone();
            r.external_id = format!("place:{i}");
            r.merchant.id = r.external_id.clone();
            r.raw = serde_json::json!({});
            r
        })
        .collect();
    let report = store.reconcile_import(inputs, false, 5000)?;
    assert_eq!(report.delta.added, 6002);
    assert_eq!(report.dedupe.candidates, 6001);
    assert!(report.dedupe.details_truncated);
    assert!(report.dedupe.groups.len() <= 5000);
    assert_eq!(store.stats()?.total, 1);
    Ok(())
}
