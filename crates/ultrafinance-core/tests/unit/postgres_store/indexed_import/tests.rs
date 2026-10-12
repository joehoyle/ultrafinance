use super::*;
fn record(i: usize) -> SourceRecord {
    serde_json::from_value(json!({"source":"stream-fixture","external_id":i.to_string(),"merchant":{"id":i.to_string(),"name":"Starbucks"},"attribution":"test","license":"test","url":"https://example.test","version":null,"raw":{}})).unwrap()
}
#[test]
fn streaming_chunks_reuse_identity_and_late_failures_roll_back() -> Result<()> {
    let store = crate::store::MerchantStore::temporary()?;
    let directory =
        std::env::temp_dir().join(format!("ultra-stream-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory)?;
    let path = directory.join("knowledge.json");
    let mut records: Vec<_> = (0..(BATCH + 1)).map(record).collect();
    std::fs::write(&path, serde_json::to_vec(&records)?)?;
    let before = serde_json::to_value((
        store.list(None, 100, 0)?,
        store.source_records("stream-fixture", None, 100, 0)?,
    ))?;
    let (preview, available, selected) = store.reconcile_file(
        path.clone(),
        None,
        true,
        crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE,
    )?;
    assert_eq!((available, selected), (BATCH + 1, BATCH + 1));
    assert_eq!(preview.dedupe.candidates, BATCH);
    assert!(preview.dedupe.details_truncated);
    assert_eq!(
        serde_json::to_value((
            store.list(None, 100, 0)?,
            store.source_records("stream-fixture", None, 100, 0)?
        ))?,
        before
    );
    assert_eq!(store.stats()?.total, 0);
    // A duplicate source key after the first completed chunk must roll back it.
    records[BATCH] = records[0].clone();
    std::fs::write(&path, serde_json::to_vec(&records)?)?;
    assert!(
        store
            .reconcile_file(
                path.clone(),
                None,
                false,
                crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE
            )
            .is_err()
    );
    assert_eq!(
        serde_json::to_value((
            store.list(None, 100, 0)?,
            store.source_records("stream-fixture", None, 100, 0)?
        ))?,
        before
    );
    records[BATCH] = record(BATCH);
    let encoded = serde_json::to_string(&records)?;
    std::fs::write(&path, format!("{encoded} trailing garbage"))?;
    assert!(
        store
            .reconcile_file(
                path.clone(),
                None,
                false,
                crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE
            )
            .is_err()
    );
    assert_eq!(
        serde_json::to_value((
            store.list(None, 100, 0)?,
            store.source_records("stream-fixture", None, 100, 0)?
        ))?,
        before
    );
    std::fs::write(&path, encoded)?;
    let (result, _, _) = store.reconcile_file(
        path.clone(),
        None,
        false,
        crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE,
    )?;
    assert_eq!(result.delta.added, BATCH + 1);
    assert_eq!(result.dedupe.candidates, BATCH);
    assert_eq!(store.stats()?.total, 1);
    let target = store.resolve_source("stream-fixture", "0")?.unwrap();
    assert_eq!(
        store.resolve_source("stream-fixture", &BATCH.to_string())?,
        Some(target)
    );
    let (refresh, _, selected) = store.reconcile_file(
        path,
        Some(2),
        false,
        crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE,
    )?;
    assert_eq!(selected, 2);
    assert_eq!(refresh.delta.unchanged, 2);
    std::fs::remove_dir_all(directory)?;
    Ok(())
}
#[test]
fn configured_chunks_preserve_dedupe_and_rollback() -> Result<()> {
    for size in [1, 2, 17, 10000] {
        let store = crate::store::MerchantStore::temporary()?;
        let records: Vec<_> = (0..19).map(record).collect();
        assert!(store.reconcile_import(records.clone(), false, 0).is_err());
        let preview = store.reconcile_import(records.clone(), true, size)?;
        assert_eq!(preview.dedupe.candidates, 18);
        assert_eq!(store.stats()?.total, 0);
        let result = store.reconcile_import(records.clone(), false, size)?;
        assert_eq!(result.delta.added, 19);
        assert_eq!(result.dedupe.candidates, 18);
        assert_eq!(store.stats()?.total, 1);
        let before = serde_json::to_value((
            store.list(None, 100, 0)?,
            store.source_records("stream-fixture", None, 100, 0)?,
        ))?;
        let mut invalid = records.clone();
        invalid.push(records[0].clone());
        assert!(store.reconcile_import(invalid, false, size).is_err());
        assert_eq!(
            serde_json::to_value((
                store.list(None, 100, 0)?,
                store.source_records("stream-fixture", None, 100, 0)?
            ))?,
            before
        );
        assert_eq!(
            store
                .reconcile_import(records, false, size)?
                .delta
                .unchanged,
            19
        );
    }
    Ok(())
}
#[tokio::test]
async fn concurrent_imports_share_indexed_identity() -> Result<()> {
    let store = crate::store::MerchantStore::temporary()?;
    let a = crate::dedupe::import(store.clone(), vec![record(0)], Default::default());
    let b = crate::dedupe::import(store.clone(), vec![record(1)], Default::default());
    let (a, b) = tokio::join!(a, b);
    a?;
    b?;
    assert_eq!(store.stats()?.total, 1);
    assert_eq!(
        store.resolve_source("stream-fixture", "0")?,
        store.resolve_source("stream-fixture", "1")?
    );
    Ok(())
}
#[test]
fn version_six_identity_backfill_crosses_bounded_pages() -> Result<()> {
    let lease = crate::store::MerchantStore::temporary()?;
    let base = lease.temporary_url().to_owned();
    let legacy = PostgresStore::connect(&base, false)?;
    legacy.run(|client| {
        client.batch_execute(include_str!("../version_12_locations.sql"))?;
        client.batch_execute("INSERT INTO merchants(id,name,website,markets_json,aliases_json,sources_json) SELECT 'legacy-'||lpad(i::text,8,'0'),'Café Brand LLC',CASE WHEN i%2=0 THEN 'https://www.brand.test/ca' ELSE NULL END,'[]','[]','[]' FROM generate_series(0,5000) i; DROP TABLE merchant_identity_keys; DROP FUNCTION advance_catalog_revision() CASCADE; DROP TABLE catalog_revision; UPDATE ultrafinance_schema SET version=6")?;
        Ok(())
    })?;
    let upgraded = PostgresStore::connect(&base, true)?;
    upgraded.run(|client| {
        let count:i64=client.query_one("SELECT count(*) FROM merchant_identity_keys",&[])?.get(0);
        assert_eq!(count,5001);
        let row=client.query_one("SELECT rule_name,rule_host FROM merchant_identity_keys WHERE merchant_id='legacy-00005000'",&[])?;
        assert_eq!(row.get::<_,String>(0),"cafe brand");assert_eq!(row.get::<_,String>(1),"brand.test");
        let row=client.query_one("SELECT rule_name,rule_host FROM merchant_identity_keys WHERE merchant_id='legacy-00004999'",&[])?;
        assert_eq!(row.get::<_,String>(0),"cafe brand llc");assert_eq!(row.get::<_,String>(1),"");
        assert_eq!(client.query_one("SELECT version FROM ultrafinance_schema",&[])?.get::<_,i32>(0),14);
        Ok(())
    })
}

#[test]
fn identity_index_tracks_corrections_and_uses_indexed_lookup() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    let mut m = record(0).merchant;
    m.id = "manual".into();
    m.name = "Café Brand, LLC".into();
    m.website = Some("https://www.brand.test/ca".into());
    store.put(&m)?;
    store.run(|client| {
        client.batch_execute("SET enable_seqscan=off")?;
        let row=client.query_one("SELECT rule_name,rule_host FROM merchant_identity_keys WHERE merchant_id='manual'",&[])?;
        assert_eq!(row.get::<_,String>(0),"cafe brand");assert_eq!(row.get::<_,String>(1),"brand.test");
        let plan=client.query("EXPLAIN SELECT merchant_id FROM merchant_identity_keys WHERE rule_name='cafe brand' AND rule_host='brand.test'",&[])?;
        assert!(plan.iter().any(|r|r.get::<_,String>(0).contains("merchant_identity_rule")));
        Ok(())
    })?;
    m.name = "Renamed".into();
    m.website = None;
    store.put(&m)?;
    store.run(|client| {
        let row = client.query_one(
            "SELECT rule_name,rule_host FROM merchant_identity_keys WHERE merchant_id='manual'",
            &[],
        )?;
        assert_eq!(row.get::<_, String>(0), "renamed");
        assert_eq!(row.get::<_, String>(1), "");
        Ok(())
    })
}
