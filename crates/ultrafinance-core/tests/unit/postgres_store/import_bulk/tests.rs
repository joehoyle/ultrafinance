use super::*;

#[test]
fn ordered_append_probes_only_each_merchants_history() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    store.run(|client| {
        let mut tx = client.transaction()?;
        // No statistics: this reproduces the BitmapAnd plan seen during the
        // growing import. Analyzed tables hide the regression in the old SQL.
        tx.batch_execute("CREATE TEMP TABLE source_records(source TEXT,external_id TEXT,merchant_id TEXT,input_website TEXT,input_logo_url TEXT,PRIMARY KEY(source,external_id)); CREATE INDEX ON source_records(merchant_id); INSERT INTO source_records SELECT 'foursquare',lpad(i::text,8,'0'),i::text,NULL,NULL FROM generate_series(1,20000) i")?;
        let ids: Vec<_> = (1..=500).map(|i| i.to_string()).collect();
        let needs = vec![false; ids.len()];
        let before = "99999999";
        let rows = tx.query(ORDERED_APPEND_PROOF, &[&ids, &needs, &needs, &before])?;
        assert_eq!(rows.len(), ids.len());
        let plan = tx.query(
            &format!("EXPLAIN {ORDERED_APPEND_PROOF}"),
            &[&ids, &needs, &needs, &before],
        )?;
        let plan: String = plan.iter().map(|r| r.get::<_, String>(0)).collect();
        // In particular, never build a source-wide external-ID bitmap per
        // merchant. All source index access must be by merchant_id.
        assert!(plan.contains("source_records_merchant_id_idx"), "{plan}");
        assert!(!plan.contains("source_records_pkey"), "{plan}");
        assert!(!plan.contains("BitmapAnd"), "{plan}");
        let website_required = vec![true; ids.len()];
        assert!(tx.query(ORDERED_APPEND_PROOF, &[&ids, &website_required, &needs, &before])?.is_empty());
        let before = "00000000";
        assert!(tx.query(ORDERED_APPEND_PROOF, &[&ids, &needs, &needs, &before])?.is_empty());
        Ok(())
    })
}

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
        client.batch_execute("CREATE TABLE merchant_writes(id TEXT,operation TEXT,aliases TEXT); CREATE FUNCTION record_merchant_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO merchant_writes VALUES(NEW.id,TG_OP,NEW.aliases_json); RETURN NEW; END $$; CREATE TRIGGER merchant_write AFTER INSERT OR UPDATE ON merchants FOR EACH ROW EXECUTE FUNCTION record_merchant_write()")?;
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
fn append_only_place_reuse_requires_identical_canonical_evidence() -> Result<()> {
    let record: SourceRecord = serde_json::from_value(serde_json::json!({
        "source":"foursquare", "external_id":"place:two",
        "merchant":{"id":"input", "name":"Alpha", "markets":["CA"],
            "website":"https://alpha.test", "aliases":["Alpha"], "sources":["https://source.test"]},
        "attribution":"test", "license":"test", "url":"https://source.test", "raw":{}
    }))?;
    let mut merchant = record.merchant.clone();
    merchant.id = "canonical".into();
    assert!(redundant_place_evidence(&merchant, &record));
    // A new source sorting first still fills a missing website from retained evidence.
    let mut without_site = record.clone();
    without_site.merchant.website = None;
    assert!(redundant_place_evidence(&merchant, &without_site));
    let mut cases = Vec::new();
    let mut changed = record.clone();
    changed.merchant.name = "Other".into();
    cases.push(changed);
    let mut changed = record.clone();
    changed.merchant.website = Some("https://other.test".into());
    cases.push(changed);
    let mut changed = record.clone();
    changed.merchant.aliases.push("NEW ALIAS".into());
    cases.push(changed);
    let mut changed = record.clone();
    changed.merchant.markets.push("US".into());
    cases.push(changed);
    let mut changed = record.clone();
    changed.raw = serde_json::json!({"countryHints":["US"]});
    cases.push(changed);
    let mut changed = record.clone();
    changed.raw = serde_json::json!({"places":[{"country":"US"}]});
    cases.push(changed);
    let mut changed = record.clone();
    changed.url = "https://new-source.test".into();
    cases.push(changed);
    let mut changed = record.clone();
    changed.source = "other".into();
    cases.push(changed);
    let mut changed = record.clone();
    changed.merchant.logo_url = Some("https://alpha.test/logo.png".into());
    cases.push(changed);
    for changed in cases {
        assert!(
            !redundant_place_evidence(&merchant, &changed),
            "{changed:?}"
        );
    }
    Ok(())
}

#[test]
fn append_only_place_reuse_matches_full_rebuild_and_refresh() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
    let optimized = PostgresStore::temporary(&base)?;
    let reference = PostgresStore::temporary(&base)?;
    let make =
        |external: &str, name: &str, country: &str, site: Option<&str>| -> Result<SourceRecord> {
            Ok(serde_json::from_value(serde_json::json!({
                "source":"foursquare", "external_id":external,
                "merchant":{"id":external, "name":name, "markets":[country],"website":site},
                "attribution":"test", "license":"test", "url":"https://source.test", "raw":{}
            }))?)
        };
    let seed = make("middle", "Alpha", "CA", Some("https://alpha.test"))?;
    let mut case_only = make("z1-case", "Alpha", "CA", None)?;
    case_only.merchant.aliases = vec!["ALPHA".into()];
    let chunks = vec![
        (vec![seed], false),
        (
            vec![make("z1", "Alpha", "CA", Some("https://alpha.test"))?],
            true,
        ),
        (vec![make("a0", "Alpha", "CA", None)?], true),
        (vec![case_only], true),
        // One novel contribution must invalidate reuse for this entire merchant.
        (
            vec![
                make("z2", "Alpha", "CA", None)?,
                make("z3", "Alpha", "US", None)?,
            ],
            true,
        ),
        // Earlier sources can change the canonical name and first website.
        (
            vec![make("000", "New name", "CA", Some("https://new.test"))?],
            true,
        ),
        // Replacements must always rebuild, including removal of country evidence.
        (vec![make("z3", "Alpha", "CA", None)?], false),
    ];
    for (step, (records, append)) in chunks.into_iter().enumerate() {
        if matches!(step, 1..=3) {
            optimized.run(|client| {
                client.batch_execute("CREATE FUNCTION reject_redundant_index_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'unchanged canonical fields must retain search and aliases'; END $$; CREATE TRIGGER no_alias_refresh BEFORE INSERT OR UPDATE OR DELETE ON aliases FOR EACH ROW EXECUTE FUNCTION reject_redundant_index_write(); CREATE TRIGGER no_search_refresh BEFORE INSERT OR UPDATE OR DELETE ON merchant_search FOR EACH ROW EXECUTE FUNCTION reject_redundant_index_write()")?;
                Ok(())
            })?;
        }
        for (store, fast) in [(&optimized, append), (&reference, false)] {
            let baseline = store.dedupe_snapshot()?;
            let records = records.clone();
            let identities = vec!["canonical".to_owned(); records.len()];
            store.run(move |client| {
                let mut tx = client.transaction()?;
                if fast {
                    import_new_sources(&mut tx, &records, &identities, &baseline)?;
                } else {
                    import(&mut tx, &records, &identities, &baseline)?;
                }
                tx.commit()?;
                Ok(())
            })?;
        }
        if matches!(step, 1..=3) {
            optimized.run(|client| {
                client.batch_execute("DROP TRIGGER no_alias_refresh ON aliases; DROP TRIGGER no_search_refresh ON merchant_search; DROP FUNCTION reject_redundant_index_write()")?;
                Ok(())
            })?;
        }
        assert_eq!(
            serde_json::to_value(optimized.get("canonical")?.unwrap())?,
            serde_json::to_value(reference.get("canonical")?.unwrap())?
        );
        for table in [
            "aliases",
            "merchant_search",
            "merchant_identity_keys",
            "source_records",
        ] {
            let read = |store: &PostgresStore| -> Result<Vec<String>> {
                let sql = format!("SELECT row_to_json(t)::text FROM {table} t ORDER BY 1");
                store.run(move |client| {
                    Ok(client.query(&sql, &[])?.iter().map(|r| r.get(0)).collect())
                })
            };
            assert_eq!(read(&optimized)?, read(&reference)?, "{table}");
        }
    }
    assert_eq!(optimized.get("canonical")?.unwrap().markets, ["CA"]);
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

#[test]
fn ordered_foursquare_append_matches_full_rebuild_for_novel_evidence() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
    let optimized = PostgresStore::temporary(&base)?;
    let reference = PostgresStore::temporary(&base)?;
    let make = |external: &str, name: &str, country: &str, site: Option<&str>| -> SourceRecord {
        serde_json::from_value(serde_json::json!({
            "source":"foursquare", "external_id":external,
            "merchant":{"id":external,"name":name,"markets":[country],"website":site},
            "attribution":"test","license":"test","url":format!("https://source.test/{external}"),"raw":{}
        })).unwrap()
    };
    let mut first_new = make("z10", "Another name", "CA", Some("https://first-new.test"));
    first_new.merchant.aliases = vec!["NEW ALIAS".into()];
    first_new.merchant.logo_url = Some("https://first-new.test/logo.png".into());
    first_new.merchant.logo_source = Some("first-new".into());
    first_new.raw = serde_json::json!({"countryHints":["GB"]});
    let mut replacement = first_new.clone();
    replacement.merchant.website = Some("https://refreshed.test".into());
    replacement.merchant.logo_url = None;
    replacement.merchant.logo_source = None;
    replacement.raw = serde_json::json!({});
    let mut other_source = make("other-existing", "Original name", "CA", None);
    other_source.source = "aaa-other-source".into();
    let chunks = vec![
        (
            vec![
                other_source,
                make("m00", "Original name", "CA", None),
                make(
                    "zzzz",
                    "Existing later source",
                    "CA",
                    Some("https://old-tail.test"),
                ),
            ],
            false,
        ),
        // Deliberately reversed: PostgreSQL source order must choose z10's website/logo.
        (
            vec![
                make("z20", "Other name", "US", Some("https://later.test")),
                first_new,
            ],
            true,
        ),
        (
            vec![
                make("zz10", "Yet more aliases", "IT", None),
                make("zz00", "More aliases", "DE", Some("https://another.test")),
            ],
            true,
        ),
        (vec![replacement], false),
        // An earlier new identity must fall back and change canonical precedence.
        (
            vec![make(
                "a00",
                "Earlier name",
                "FR",
                Some("https://earlier.test"),
            )],
            true,
        ),
        (vec![make("zz20", "Last name", "JP", None)], true),
    ];
    for (step, (records, append)) in chunks.into_iter().enumerate() {
        for (store, fast) in [(&optimized, append), (&reference, false)] {
            let baseline = store.dedupe_snapshot()?;
            let records = records.clone();
            store.run(move |client| {
                let mut tx = client.transaction()?;
                if fast && step == 1 {
                    let canonical = baseline
                        .merchants
                        .iter()
                        .map(|m| (m.id.as_str(), m))
                        .collect();
                    // The retained website sorts later: a new earlier website
                    // can replace it, so preserving that canonical value is unsafe.
                    assert!(ordered_foursquare_append(&mut tx, &records, &canonical)?.is_none());
                }
                if fast && step == 2 {
                    let canonical = baseline
                        .merchants
                        .iter()
                        .map(|m| (m.id.as_str(), m))
                        .collect();
                    assert_eq!(
                        ordered_foursquare_append(&mut tx, &records, &canonical)?.map(|v| v.order),
                        Some(vec![1, 0])
                    );
                }
                let identities = vec!["canonical".into(); records.len()];
                if fast {
                    import_new_sources(&mut tx, &records, &identities, &baseline)?;
                } else {
                    import(&mut tx, &records, &identities, &baseline)?;
                }
                tx.commit()?;
                Ok(())
            })?;
        }
        assert_eq!(
            serde_json::to_value(optimized.get("canonical")?.unwrap())?,
            serde_json::to_value(reference.get("canonical")?.unwrap())?,
            "step {step}"
        );
        for table in [
            "aliases",
            "merchant_search",
            "merchant_identity_keys",
            "source_records",
        ] {
            let read = |store: &PostgresStore| -> Result<Vec<String>> {
                let sql = format!("SELECT row_to_json(t)::text FROM {table} t ORDER BY 1");
                store.run(move |client| {
                    Ok(client.query(&sql, &[])?.iter().map(|r| r.get(0)).collect())
                })
            };
            assert_eq!(read(&optimized)?, read(&reference)?, "step {step}: {table}");
        }
        if step == 1 {
            let m = optimized.get("canonical")?.unwrap();
            assert_eq!(m.name, "Original name");
            assert_eq!(m.website.as_deref(), Some("https://first-new.test"));
            assert_eq!(m.logo_source.as_deref(), Some("first-new"));
            assert_eq!(m.markets, ["CA", "GB", "US"]);
        }
    }
    // The retained aaa-other-source still precedes every Foursquare source.
    assert_eq!(optimized.get("canonical")?.unwrap().name, "Original name");
    Ok(())
}

#[test]
fn ordered_foursquare_append_limits_manual_veto_to_the_target_merchant() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
    let make = |source: &str, external: &str| -> SourceRecord {
        serde_json::from_value(serde_json::json!({
            "source":source,"external_id":external,
            "merchant":{"id":external,"name":format!("Brand {external}")},
            "attribution":"test","license":"test","url":"https://source.test","raw":{}
        }))
        .unwrap()
    };
    for kind in ["manual", "aaa-other-source", "zzz-other-source"] {
        let store = PostgresStore::temporary(&base)?;
        store.reconcile_import(vec![make("foursquare", "middle")], false, 5000)?;
        let pending = vec![make("foursquare", "zz-new")];
        let probe = |store: &PostgresStore, records: Vec<SourceRecord>| -> Result<bool> {
            let baseline = store.dedupe_snapshot()?;
            store.run(move |client| {
                let mut tx = client.transaction()?;
                let canonical = baseline
                    .merchants
                    .iter()
                    .map(|m| (m.id.as_str(), m))
                    .collect();
                Ok(ordered_foursquare_append(&mut tx, &records, &canonical)?.is_some())
            })
        };
        assert!(probe(&store, pending.clone())?);
        assert!(!probe(&store, vec![make("foursquare", "a-earlier")])?);
        assert!(!probe(&store, vec![make("other", "zz-new")])?);
        if kind == "manual" {
            let id = store.resolve_source("foursquare", "middle")?.unwrap();
            let mut manual = store.get(&id)?.unwrap();
            manual.name = "Manual name".into();
            store.put(&manual)?;
            assert!(!probe(&store, pending)?);
        } else {
            store.reconcile_import(vec![make(kind, "other-existing")], false, 5000)?;
            assert!(probe(&store, pending)?);
        }
    }
    Ok(())
}
