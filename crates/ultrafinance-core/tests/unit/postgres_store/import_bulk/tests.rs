use super::*;

#[test]
fn copy_roundtrips_null_unicode_empty_and_large_text() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    store.run(|client| {
        let mut tx = client.transaction()?;
        tx.batch_execute("CREATE TEMP TABLE copy_roundtrip(a TEXT,b TEXT)")?;
        let rows = vec![
            vec![None, Some(String::new())],
            vec![
                Some("Café 東京\n\t\\\"".into()),
                Some("x".repeat(2 * 1024 * 1024)),
            ],
        ];
        copy_rows(
            &mut tx,
            "COPY copy_roundtrip(a,b) FROM STDIN BINARY",
            2,
            &rows,
        )?;
        let actual: Vec<Vec<Option<String>>> = tx
            .query("SELECT a,b FROM copy_roundtrip ORDER BY a NULLS FIRST", &[])?
            .iter()
            .map(|r| vec![r.get(0), r.get(1)])
            .collect();
        assert_eq!(actual, rows);
        tx.commit()?;
        Ok(())
    })
}
#[test]
fn fresh_merchants_insert_final_fields_once_across_source_batches() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    store.run(|client| {
        client.batch_execute("CREATE TABLE merchant_writes(id TEXT,operation TEXT,aliases TEXT); CREATE FUNCTION record_merchant_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO merchant_writes VALUES(NEW.id,TG_OP,to_json(NEW.aliases)::text); RETURN NEW; END $$; CREATE TRIGGER merchant_write AFTER INSERT OR UPDATE ON merchants FOR EACH ROW EXECUTE FUNCTION record_merchant_write()")?;
        Ok(())
    })?;
    let records: Vec<SourceRecord> =
        (0..BATCH + 1)
            .map(|i| {
                SourceRecord {
        source:"single-write".into(),external_id:format!("{i:08}"),
        merchant:serde_json::from_value(serde_json::json!({
            "id":format!("input-{i}"),"name":format!("Merchant {}",if i==BATCH {0} else {i}),
            "aliases":[if i==BATCH {"BANK LAST"} else {"BANK FIRST"}],
            "markets":[if i==BATCH {"US"} else {"CA"}]
        })).unwrap(),
        attribution:"test".into(),license:"test".into(),url:format!("https://example.org/{i}"),
        version:None,raw:serde_json::json!({}),
    }
            })
            .collect();
    let ids: Vec<_> = (0..BATCH + 1)
        .map(|i| format!("canonical-{}", if i == BATCH { 0 } else { i }))
        .collect();
    let expected = store.dedupe_snapshot()?;
    let mut staged = expected.clone();
    staged.merchants = records[..BATCH]
        .iter()
        .zip(&ids)
        .map(|(r, id)| {
            let mut m = r.merchant.clone();
            m.id = id.clone();
            m
        })
        .collect();
    staged.sources = ids.iter().cloned().zip(records.iter().cloned()).collect();
    let (delta, _) = store.apply_reconciled_import(&expected, &staged, &records, &ids, &[])?;
    assert_eq!(delta.added, BATCH + 1);
    let first = store.get(&ids[0])?.unwrap();
    assert!(first.aliases.contains(&"BANK FIRST".into()));
    assert!(first.aliases.contains(&"BANK LAST".into()));
    assert_eq!(first.markets, vec!["CA", "US"]);
    assert_eq!(first.sources.len(), 2);
    store.run(|client| {
        assert_eq!(
            client
                .query_one(
                    "SELECT count(*) FROM merchant_writes WHERE operation='INSERT'",
                    &[]
                )?
                .get::<_, i64>(0),
            BATCH as i64
        );
        assert_eq!(
            client
                .query_one(
                    "SELECT count(*) FROM merchant_writes WHERE operation='UPDATE'",
                    &[]
                )?
                .get::<_, i64>(0),
            0
        );
        let inserted: String = client
            .query_one(
                "SELECT aliases FROM merchant_writes WHERE id='canonical-0'",
                &[],
            )?
            .get(0);
        assert!(serde_json::from_str::<Vec<String>>(&inserted)?.contains(&"BANK LAST".into()));
        Ok(())
    })?;
    // A real refresh of an established merchant must still update it.
    let mut refresh = records[BATCH].clone();
    refresh.merchant.aliases = vec!["BANK LAST CHANGED".into()];
    let expected = store.dedupe_snapshot()?;
    let (delta, _) =
        store.apply_reconciled_import(&expected, &expected, &[refresh], &[ids[0].clone()], &[])?;
    assert_eq!(delta.updated, 1);
    assert!(
        store
            .get(&ids[0])?
            .unwrap()
            .aliases
            .contains(&"BANK LAST CHANGED".into())
    );
    store.run(|client| {
        assert_eq!(
            client
                .query_one(
                    "SELECT count(*) FROM merchant_writes WHERE operation='UPDATE'",
                    &[]
                )?
                .get::<_, i64>(0),
            1
        );
        Ok(())
    })?;
    Ok(())
}

#[test]
fn search_and_alias_indexes_only_change_when_searchable_names_change() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    let mut record: SourceRecord = serde_json::from_value(serde_json::json!({
        "source":"search-write-test", "external_id":"one",
        "merchant":{"id":"input", "name":"Alpha Café"},
        "attribution":"test", "license":"test", "url":"https://example.test", "raw":{}
    }))?;
    store.reconcile_import(vec![record.clone()], false, 5000)?;
    store.run(|client| {
        client.batch_execute("CREATE TABLE search_writes(operation TEXT); CREATE FUNCTION record_search_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO search_writes VALUES(TG_OP); RETURN NEW; END $$; CREATE TRIGGER search_write AFTER INSERT OR UPDATE OR DELETE ON merchant_search FOR EACH ROW EXECUTE FUNCTION record_search_write()")?;
        client.batch_execute("CREATE TABLE alias_writes(operation TEXT); CREATE FUNCTION record_alias_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO alias_writes VALUES(TG_OP); RETURN NULL; END $$; CREATE TRIGGER alias_write AFTER INSERT OR UPDATE OR DELETE ON aliases FOR EACH ROW EXECUTE FUNCTION record_alias_write()")?;
        Ok(())
    })?;
    // A changed source input rebuilds coverage/identity, but not unchanged search text.
    record.merchant.website = Some("https://alpha.example".into());
    assert_eq!(
        store
            .reconcile_import(vec![record.clone()], false, 5000)?
            .delta
            .updated,
        1
    );
    store.run(|client| {
        assert_eq!(
            client
                .query_one("SELECT count(*) FROM search_writes", &[])?
                .get::<_, i64>(0),
            0
        );
        assert_eq!(
            client
                .query_one("SELECT count(*) FROM alias_writes", &[])?
                .get::<_, i64>(0),
            0
        );
        Ok(())
    })?;
    record.merchant.name = "Beta Café".into();
    store.reconcile_import(vec![record], false, 5000)?;
    store.run(|client| {
        assert_eq!(
            client
                .query_one("SELECT operation FROM search_writes", &[])?
                .get::<_, String>(0),
            "UPDATE"
        );
        assert_eq!(
            client
                .query_one("SELECT text FROM merchant_search", &[])?
                .get::<_, String>(0),
            "beta cafe"
        );
        assert_eq!(
            client
                .query_one("SELECT normalized FROM aliases", &[])?
                .get::<_, String>(0),
            "beta cafe"
        );
        let operations: Vec<String> = client
            .query("SELECT operation FROM alias_writes ORDER BY operation", &[])?
            .iter()
            .map(|row| row.get(0))
            .collect();
        assert_eq!(operations, ["DELETE", "INSERT"]);
        Ok(())
    })?;
    Ok(())
}
#[test]
fn later_search_batch_failure_rolls_back_source_copies_and_indexes() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    store.run(|client| {
        client.batch_execute("ALTER TABLE merchant_search ADD CONSTRAINT import_test_failure CHECK (text <> 'forced failure')")?;
        Ok(())
    })?;
    let expected = store.dedupe_snapshot()?;
    let records: Vec<SourceRecord> = (0..BATCH+1).map(|i| SourceRecord {
        source: "bulk-rollback".into(),external_id:i.to_string(),
        merchant: serde_json::from_value(serde_json::json!({"id":format!("id-{i}"),"name":if i==BATCH {"Forced failure".into()} else {format!("Merchant {i}")},"markets":["CA"]})).unwrap(),
        attribution:"test".into(),license:"test".into(),url:"https://example.org".into(),version:None,raw:serde_json::json!({}),
    }).collect();
    let ids: Vec<_> = records.iter().map(|r| r.merchant.id.clone()).collect();
    let mut staged = expected.clone();
    staged.merchants = records.iter().map(|r| r.merchant.clone()).collect();
    staged.sources = ids.iter().cloned().zip(records.iter().cloned()).collect();
    let result = store.apply_reconciled_import(&expected, &staged, &records, &ids, &[]);
    assert!(result.is_err());
    let after = store.dedupe_snapshot()?;
    assert!(after.merchants.is_empty());
    assert!(after.sources.is_empty());
    store.run(|client| {
        for table in ["aliases", "merchant_search", "merchant_search_words"] {
            let count: i64 = client
                .query_one(&format!("SELECT count(*) FROM {table}"), &[])?
                .get(0);
            assert_eq!(count, 0);
        }
        Ok(())
    })?;
    Ok(())
}

#[test]
fn indexed_chunks_write_keys_once_and_still_repair_unchanged_sources() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    store.run(|client| {
        client.batch_execute("CREATE TABLE key_attempts(merchant_id TEXT); CREATE FUNCTION record_key_attempt() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO key_attempts VALUES(NEW.merchant_id); RETURN NEW; END $$; CREATE TRIGGER key_attempt BEFORE INSERT ON merchant_identity_keys FOR EACH ROW EXECUTE FUNCTION record_key_attempt()")?;
        Ok(())
    })?;
    let mut record: SourceRecord = serde_json::from_value(serde_json::json!({
        "source":"key-attempt-test","external_id":"one",
        "merchant":{"id":"input","name":"Alpha Café","markets":["CA"]},
        "attribution":"test","license":"test","url":"https://example.test","raw":{}
    }))?;
    store.reconcile_import(vec![record.clone()], false, 5000)?;
    let id = store.resolve_source("key-attempt-test", "one")?.unwrap();
    store.run(|client| {
        assert_eq!(
            client
                .query_one("SELECT count(*) FROM key_attempts", &[])?
                .get::<_, i64>(0),
            1
        );
        client.batch_execute("TRUNCATE key_attempts")?;
        Ok(())
    })?;
    record.merchant.website = Some("https://alpha.example".into());
    assert_eq!(
        store
            .reconcile_import(vec![record.clone()], false, 5000)?
            .delta
            .updated,
        1
    );
    let expected = crate::dedupe::deterministic_key(&store.get(&id)?.unwrap()).unwrap();
    let corrupt_id = id.clone();
    store.run(move |client| {
        assert_eq!(client.query_one("SELECT count(*) FROM key_attempts", &[])?.get::<_, i64>(0), 1);
        client.execute("UPDATE merchant_identity_keys SET rule_name='obsolete',rule_host='obsolete' WHERE merchant_id=$1", &[&corrupt_id])?;
        client.batch_execute("TRUNCATE key_attempts")?;
        Ok(())
    })?;
    assert_eq!(
        store
            .reconcile_import(vec![record], false, 5000)?
            .delta
            .unchanged,
        1
    );
    store.run(move |client| {
        assert_eq!(
            client
                .query_one("SELECT count(*) FROM key_attempts", &[])?
                .get::<_, i64>(0),
            1
        );
        let row = client.query_one(
            "SELECT rule_name,rule_host FROM merchant_identity_keys WHERE merchant_id=$1",
            &[&id],
        )?;
        assert_eq!(row.get::<_, String>(0), expected.0);
        assert_eq!(row.get::<_, String>(1), expected.1.unwrap_or_default());
        Ok(())
    })
}

#[test]
fn changed_existing_identity_is_indexed_before_matching_fresh_sources() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    let mut existing: SourceRecord = serde_json::from_value(serde_json::json!({
        "source":"refresh-match-test","external_id":"old",
        "merchant":{"id":"old-input","name":"Old Name","website":"https://old.example","markets":["CA"]},
        "attribution":"test","license":"test","url":"https://example.test","raw":{}
    }))?;
    store.reconcile_import(vec![existing.clone()], false, 5000)?;
    let id = store.resolve_source("refresh-match-test", "old")?.unwrap();
    existing.merchant.name = "New Brand".into();
    existing.merchant.website = Some("https://brand.example".into());
    let mut fresh = existing.clone();
    fresh.external_id = "fresh".into();
    fresh.merchant.id = "fresh-input".into();
    fresh.merchant.markets = vec!["US".into()];
    let result = store.reconcile_import(vec![existing, fresh], false, 5000)?;
    assert_eq!(result.delta.updated, 1);
    assert_eq!(result.delta.added, 1);
    assert_eq!(
        store.resolve_source("refresh-match-test", "fresh")?,
        Some(id.clone())
    );
    assert_eq!(store.stats()?.total, 1);
    let merchant = store.get(&id)?.unwrap();
    assert_eq!(merchant.name, "New Brand");
    assert_eq!(merchant.markets, vec!["CA", "US"]);
    Ok(())
}

#[test]
fn repaired_existing_keys_still_merge_with_new_only_buckets() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    let record: SourceRecord = serde_json::from_value(serde_json::json!({
        "source":"bucket-test", "external_id":"existing",
        "merchant":{"id":"input", "name":"Alpha Café"},
        "attribution":"test", "license":"test", "url":"https://source.test", "raw":{}
    }))?;
    store.reconcile_import(vec![record.clone()], false, 5000)?;
    store.run(|client| {
        client.batch_execute(
            "UPDATE merchant_identity_keys SET rule_name='obsolete',rule_host='obsolete'",
        )?;
        Ok(())
    })?;
    let mut fresh = record.clone();
    fresh.external_id = "new".into();
    let report = store.reconcile_import(vec![record, fresh], false, 5000)?;
    assert!(report.dedupe.candidates > 0);
    assert_eq!(store.stats()?.total, 1);
    assert_eq!(
        store.resolve_source("bucket-test", "existing")?,
        store.resolve_source("bucket-test", "new")?
    );
    Ok(())
}

#[test]
fn fresh_sources_still_consolidate_legacy_duplicate_buckets() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    let make = |external: &str| -> SourceRecord {
        serde_json::from_value(serde_json::json!({
            "source":"legacy-bucket-test", "external_id":external,
            "merchant":{"id":external, "name":"Alpha Café"},
            "attribution":"test", "license":"test", "url":"https://source.test", "raw":{}
        }))
        .unwrap()
    };
    let records = vec![make("one"), make("two")];
    let baseline = store.dedupe_snapshot()?;
    store.run(move |client| {
        let mut tx = client.transaction()?;
        import(
            &mut tx,
            &records,
            &["legacy-one".into(), "legacy-two".into()],
            &baseline,
        )?;
        tx.commit()?;
        Ok(())
    })?;
    assert_eq!(store.stats()?.total, 2);
    let report = store.reconcile_import(vec![make("three")], false, 5000)?;
    assert!(report.dedupe.candidates > 0);
    assert_eq!(store.stats()?.total, 1);
    let id = store.resolve_source("legacy-bucket-test", "one")?;
    assert_eq!(id, store.resolve_source("legacy-bucket-test", "two")?);
    assert_eq!(id, store.resolve_source("legacy-bucket-test", "three")?);
    Ok(())
}

#[test]
fn new_only_buckets_still_respect_multiple_manual_anchors() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    let make = |external: &str| -> SourceRecord {
        serde_json::from_value(serde_json::json!({
            "source":"manual-veto-test", "external_id":external,
            "merchant":{"id":external, "name":"Alpha Café"},
            "attribution":"test", "license":"test", "url":"https://source.test", "raw":{}
        }))
        .unwrap()
    };
    for id in ["manual-one", "manual-two"] {
        store.put(&make(id).merchant)?;
    }
    let report = store.reconcile_import(vec![make("one"), make("two")], false, 5000)?;
    assert_eq!(report.dedupe.candidates, 0);
    assert_eq!(store.stats()?.total, 4);
    assert_ne!(
        store.resolve_source("manual-veto-test", "one")?,
        store.resolve_source("manual-veto-test", "two")?
    );
    // Revisiting this now-established bucket must retain the same veto.
    store.reconcile_import(vec![make("one")], false, 5000)?;
    assert_eq!(store.stats()?.total, 4);
    Ok(())
}
