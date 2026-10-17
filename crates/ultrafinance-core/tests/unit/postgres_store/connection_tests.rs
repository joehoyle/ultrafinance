use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn import_working_memory_does_not_leak_to_other_transactions() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    store.run(|client| {
        client.batch_execute("SET work_mem='8MB'")?;
        for commit in [true, false] {
            let mut tx = import_transaction(client)?;
            assert_eq!(
                tx.query_one("SHOW work_mem", &[])?.get::<_, String>(0),
                "64MB"
            );
            if commit {
                tx.commit()?;
            } else {
                tx.rollback()?;
            }
            assert_eq!(
                client.query_one("SHOW work_mem", &[])?.get::<_, String>(0),
                "8MB"
            );
        }
        Ok(())
    })
}

#[test]
fn version_eleven_import_host_index_preserves_catalog_and_supports_host_lookup() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
    let lease = PostgresStore::temporary(&base)?;
    let url = lease.temporary_url().unwrap().to_owned();
    let records = crate::import::merchant_studio(
        r#"{"schemaVersion":"1.0","merchants":[{"id":"alpha","canonicalName":"Alpha","website":"https://alpha.test"},{"id":"directory","canonicalName":"Cafe","website":"https://yp.ca/cafe"}]}"#,
    )?;
    lease.import(&records)?;
    let id = lease.resolve_source("merchant-studio", "alpha")?;
    assert!(id.is_some());
    let revision: i64 = lease.run(|client| {
        client.batch_execute(include_str!("version_14_json.sql"))?;
        client.batch_execute(include_str!("version_12_locations.sql"))?;
        client.batch_execute(
            "DROP INDEX merchant_identity_rule_host; UPDATE ultrafinance_schema SET version=11; UPDATE merchant_identity_keys SET rule_name='cafe',rule_host='yp.ca' WHERE normalized_name='cafe'",
        )?;
        Ok(client
            .query_one("SELECT revision FROM catalog_revision", &[])?
            .get(0))
    })?;
    assert!(PostgresStore::connect(&url, false).is_err());
    let upgraded = PostgresStore::connect(&url, true)?;
    assert_eq!(upgraded.resolve_source("merchant-studio", "alpha")?, id);
    upgraded.run(move |client| {
        assert!(client.query_one("SELECT revision FROM catalog_revision", &[])?.get::<_, i64>(0) >= revision);
        assert_eq!(client.query_one("SELECT count(*) FROM merchants", &[])?.get::<_, i64>(0), 2);
        assert!(client.query_one("SELECT indisvalid FROM pg_index WHERE indexrelid='merchant_identity_rule_host'::regclass", &[])?.get::<_, bool>(0));
        assert!(client.query_one("SELECT rule_host IS NULL FROM merchant_identity_keys WHERE normalized_name='cafe'", &[])?.get::<_, bool>(0));
        assert_eq!(client.query_one("SELECT count(*) FROM merchant_identity_keys WHERE rule_host='alpha.test'", &[])?.get::<_, i64>(0), 1);
        Ok(())
    })
}

#[test]
fn version_nine_merge_audits_are_removed_without_losing_identity() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
    let lease = PostgresStore::temporary(&base)?;
    let url = lease.temporary_url().unwrap().to_owned();
    lease.run(|client| {
        assert!(client.query_one(
            "SELECT to_regclass('merchant_merge_runs') IS NULL AND to_regclass('location_merge_runs') IS NULL",
            &[],
        )?.get::<_, bool>(0));
        client.batch_execute(include_str!("version_14_json.sql"))?;
        client.batch_execute(include_str!("version_12_locations.sql"))?;
        client.batch_execute("CREATE TABLE merchant_merge_runs(id TEXT PRIMARY KEY,data TEXT NOT NULL); CREATE TABLE location_merge_runs(id TEXT PRIMARY KEY,data TEXT NOT NULL); INSERT INTO merchant_merge_runs VALUES('merchant-audit','{}'); INSERT INTO location_merge_runs VALUES('location-audit','{}'); INSERT INTO merchant_redirects VALUES('retired','survivor'); UPDATE ultrafinance_schema SET version=9")?;
        Ok(())
    })?;
    assert!(PostgresStore::connect(&url, false).is_err());
    let upgraded = PostgresStore::connect(&url, true)?;
    assert_eq!(upgraded.resolve_merchant_id("retired")?, "survivor");
    // Reinitialization must not recreate either audit table.
    let initialized = PostgresStore::connect(&url, true)?;
    initialized.run(|client| {
        assert_eq!(client.query_one("SELECT version FROM ultrafinance_schema", &[])?.get::<_, i32>(0), 18);
        assert!(client.query_one(
            "SELECT to_regclass('merchant_merge_runs') IS NULL AND to_regclass('location_merge_runs') IS NULL",
            &[],
        )?.get::<_, bool>(0));
        Ok(())
    })
}

#[test]
fn version_eight_legacy_views_are_migrated_before_import() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
    let lease = PostgresStore::temporary(&base)?;
    let url = lease.temporary_url().unwrap().to_owned();
    let records = crate::import::merchant_studio(
        r#"{"schemaVersion":"1.0","merchants":[{"id":"alpha","canonicalName":"Alpha","countryHints":["CA"]}]}"#,
    )?;
    lease.run(move |client| {
        let mut tx = client.transaction()?;
        let data = serde_json::to_string(&records[0].merchant)?;
        crate::columns::postgres_merchants(&mut tx, "alpha", &data)?;
        crate::columns::postgres_manual_merchants(&mut tx, "alpha", &data)?;
        crate::columns::postgres_source_records(
            &mut tx, "merchant-studio", "alpha", "alpha",
            &serde_json::to_string(&records[0])?,
        )?;
        tx.batch_execute(include_str!("version_14_json.sql"))?;
        tx.batch_execute(include_str!("version_12_locations.sql"))?;
        // Recreate the stale projections found in existing version-8 catalogs.
        tx.batch_execute(r#"
            ALTER TABLE merchants ADD COLUMN market_evidence_json TEXT DEFAULT '[]';
            ALTER TABLE manual_merchants ADD COLUMN market_evidence_json TEXT DEFAULT '[]';
            CREATE TABLE source_market_inputs (source TEXT);
            CREATE TABLE merchant_market_evidence (merchant_id TEXT);
            -- Embed the old projection directly, without inventing extra custom
            -- views that would correctly block a later column-type migration.
            DO $$ DECLARE definition text; projection text; resolutions text; BEGIN
                resolutions := pg_get_viewdef('descriptor_resolution_documents'::regclass);
                DROP VIEW descriptor_resolution_documents;
                FOREACH projection IN ARRAY ARRAY['merchants_documents','manual_merchants_documents','source_records_documents'] LOOP
                    definition := rtrim(pg_get_viewdef(projection::regclass), ';');
                    EXECUTE format('DROP VIEW %I', projection);
                    IF projection='source_records_documents' THEN
                        EXECUTE format('CREATE VIEW %I AS SELECT source,external_id,merchant_id,jsonb_set(data::jsonb,''{merchant,market_evidence}'',''[]''::jsonb)::text AS data FROM (%s) legacy', projection, definition);
                    ELSE
                        EXECUTE format('CREATE VIEW %I AS SELECT id,(data::jsonb || ''{"market_evidence":[]}''::jsonb)::text AS data FROM (%s) legacy', projection, definition);
                    END IF;
                END LOOP;
                EXECUTE 'CREATE VIEW descriptor_resolution_documents AS ' || resolutions;
            END $$;
            GRANT SELECT ON merchants_documents,manual_merchants_documents,source_records_documents TO PUBLIC;
            UPDATE ultrafinance_schema SET version=8;
        "#)?;
        tx.commit()?;
        Ok(())
    })?;

    let error = match PostgresStore::connect(&url, false) {
        Ok(_) => bail!("legacy schema must require initialization"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("database init"));
    let migrated = PostgresStore::connect(&url, true)?;
    assert_eq!(migrated.get("alpha")?.unwrap().name, "Alpha");
    migrated.run(|client| {
        assert!(client.query_one("SELECT to_regclass('source_market_inputs') IS NULL AND to_regclass('merchant_market_evidence') IS NULL AND NOT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='public' AND column_name='market_evidence_json')", &[])?.get::<_, bool>(0));
        for table in ["merchants", "manual_merchants", "source_records"] {
            assert!(client.query_one("SELECT EXISTS(SELECT 1 FROM information_schema.role_table_grants WHERE table_schema='public' AND table_name=$1 AND grantee='PUBLIC' AND privilege_type='SELECT')", &[&table])?.get::<_, bool>(0));
        }
        let row = client.query_one("SELECT s.*,p.attribution,p.license,p.url FROM source_records s JOIN source_provenance p ON p.id=s.source_provenance_id", &[])?;
        let record = rows::source(&row)?;
        assert_eq!(record.raw["countryHints"], serde_json::json!(["CA"]));
        Ok(())
    })?;
    let refreshed = crate::import::merchant_studio(
        r#"{"schemaVersion":"1.0","merchants":[{"id":"alpha","canonicalName":"Alpha Updated","countryHints":["CA"]}]}"#,
    )?;
    assert_eq!(migrated.import(&refreshed)?.updated, 1);
    assert_eq!(migrated.get("alpha")?.unwrap().name, "Alpha");
    // Reinitialization remains safe after the upgrade.
    PostgresStore::connect(&url, true)?;
    Ok(())
}

#[tokio::test]
async fn lazy_store_does_not_connect_during_startup() -> Result<()> {
    // No server exists here. Construction still succeeds inside an async
    // runtime; the public website/health can start without waking Aurora.
    let store = PostgresStore::lazy("postgresql://localhost:1/unused?sslmode=disable")?;
    assert!(store.run(|_| Ok(())).is_err());
    Ok(())
}

#[test]
fn import_transaction_survives_cpu_gaps_and_restores_request_timeouts() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    store.run(|client| {
        client.batch_execute("SET idle_in_transaction_session_timeout = '1s'")?;
        let mut tx = import_transaction(client)?;
        std::thread::sleep(Duration::from_millis(1500));
        let idle: String = tx
            .query_one("SHOW idle_in_transaction_session_timeout", &[])?
            .get(0);
        let statement: String = tx.query_one("SHOW statement_timeout", &[])?.get(0);
        assert_eq!(idle, "0");
        assert_eq!(statement, "0");
        tx.commit()?;
        let idle: String = client
            .query_one("SHOW idle_in_transaction_session_timeout", &[])?
            .get(0);
        let statement: String = client.query_one("SHOW statement_timeout", &[])?.get(0);
        assert_eq!(idle, "1s");
        assert_eq!(statement, "15s");
        Ok(())
    })
}

#[test]
fn postgres_idle_session_reconnects_before_work_without_replaying_jobs() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    let first_pid: i32 = store.run(|client| {
        let version: i32 = client
            .query_one("SELECT version FROM ultrafinance_schema", &[])?
            .get(0);
        assert_eq!(
            version, 18,
            "market migration must run before application use"
        );
        let idle: String = client.query_one("SHOW idle_session_timeout", &[])?.get(0);
        assert_eq!(idle, "1min");
        client.batch_execute("SET idle_session_timeout = '1s'")?;
        Ok(client.query_one("SELECT pg_backend_pid()", &[])?.get(0))
    })?;
    std::thread::sleep(Duration::from_secs(2));
    // Server-side expiry means a fresh backend, even though the store and
    // worker remain alive. The caller sees success on its first operation.
    let second_pid: i32 = store.run(|client| {
        let idle: String = client.query_one("SHOW idle_session_timeout", &[])?.get(0);
        assert_eq!(idle, "1min");
        Ok(client.query_one("SELECT pg_backend_pid()", &[])?.get(0))
    })?;
    assert_ne!(first_pid, second_pid);

    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    let result: Result<()> = store.run(move |client| {
        observed.fetch_add(1, Ordering::SeqCst);
        client.batch_execute(
            "CREATE TEMP TABLE completed_write(value int); INSERT INTO completed_write VALUES (1)",
        )?;
        bail!("simulated error after a completed write")
    });
    assert!(result.is_err());
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    let rows: i64 = store.run(|client| {
        Ok(client
            .query_one("SELECT count(*) FROM completed_write", &[])?
            .get(0))
    })?;
    assert_eq!(rows, 1);
    Ok(())
}

#[test]
fn version_twelve_locations_backfill_merchants_and_share_provenance() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
    let lease = PostgresStore::temporary(&base)?;
    let records = crate::import::catalog(r#"[{"id":"brand","name":"Original Brand"}]"#, "fixture")?;
    lease.import(&records)?;
    let original = lease.resolve_source("fixture", "brand")?.unwrap();
    let outlet: LocationRecord = serde_json::from_value(serde_json::json!({
        "source":"fixture", "external_id":"branch-1",
        "merchant":{"source":"fixture","external_id":"brand"},
        "location":{"name":"Branch", "precision":"outlet","address":"1 Main St","country":"CA"},
        "place_ids":{"google":"google-place","apple":"apple-place"},
        "attribution":"Fixture credit", "license":"Fixture licence", "url":"https://fixture.test/ca"
    }))?;
    let mut second = outlet.clone();
    second.external_id = "branch-2".into();
    second.location.address = Some("2 Main St".into());
    let local_merchant: Merchant = serde_json::from_value(serde_json::json!({
        "id":"manual-anchor", "name":"Manual Anchor"
    }))?;
    lease.put(&local_merchant)?;
    let mut local = outlet.clone();
    local.external_id = "branch-local".into();
    local.merchant = crate::location::MerchantReference::Local {
        merchant_id: "manual-anchor".into(),
    };
    local.location.address = Some("3 Main St".into());
    lease.import_locations(&[outlet.clone(), second, local])?;
    let local_before = lease.location_sources("manual-anchor")?;
    let work_mem_before: String =
        lease.run(|client| Ok(client.query_one("SHOW work_mem", &[])?.get(0)))?;
    let before = lease.location_sources(&original)?;
    lease.run(|client| {
        client.batch_execute(include_str!("version_14_json.sql"))?;
        client.batch_execute(include_str!("version_12_locations.sql"))?;
        client.batch_execute("GRANT SELECT ON location_records_documents TO PUBLIC")?;
        assert_eq!(
            client
                .query_one(
                    "SELECT count(*) FROM location_records WHERE merchant_id IS NULL",
                    &[]
                )?
                .get::<_, i64>(0),
            2
        );
        Ok(())
    })?;
    let upgraded = PostgresStore::connect(lease.temporary_url().unwrap(), true)?;
    assert_eq!(
        serde_json::to_value(upgraded.location_sources(&original)?)?,
        serde_json::to_value(before)?
    );
    assert_eq!(
        serde_json::to_value(upgraded.location_sources("manual-anchor")?)?,
        serde_json::to_value(local_before)?
    );
    upgraded.run(move |client| {
        assert_eq!(client.query_one("SHOW work_mem", &[])?.get::<_, String>(0), work_mem_before);
        assert_eq!(client.query_one("SELECT count(*) FROM location_records WHERE merchant_id=$1", &[&original])?.get::<_, i64>(0), 2);
        assert_eq!(client.query_one("SELECT count(DISTINCT source_provenance_id) FROM location_records", &[])?.get::<_, i64>(0), 1);
        assert_eq!(client.query_one("SELECT count(*) FROM information_schema.columns WHERE table_schema='public' AND table_name='location_records' AND column_name IN ('attribution','license','url')", &[])?.get::<_, i64>(0), 0);
        assert!(client.query_one("SELECT EXISTS(SELECT 1 FROM information_schema.role_table_grants WHERE table_schema='public' AND table_name='location_records' AND grantee='PUBLIC' AND privilege_type='SELECT')", &[])?.get::<_, bool>(0));
        assert!(client.execute("UPDATE location_records SET merchant_id=NULL,merchant_source=NULL,merchant_external_id=NULL", &[]).is_err());
        assert_eq!(client.query_one("SELECT count(*) FROM pg_constraint WHERE conrelid='location_records'::regclass AND conname IN ('location_records_merchant_id_fkey','location_records_provenance_fk') AND convalidated", &[])?.get::<_, i64>(0), 2);
        let invalid = client.execute("UPDATE location_records SET merchant_id='missing' WHERE merchant_source IS NULL", &[]).unwrap_err();
        assert_eq!(invalid.as_db_error().unwrap().code(), &postgres::error::SqlState::FOREIGN_KEY_VIOLATION);
        Ok(())
    })?;
    let target: Merchant =
        serde_json::from_value(serde_json::json!({"id":"target","name":"Target Brand"}))?;
    upgraded.put(&target)?;
    upgraded.link("fixture", "brand", "target")?;
    assert_eq!(upgraded.location_sources("target")?.len(), 2);
    upgraded.run(|client| {
        assert_eq!(
            client
                .query_one(
                    "SELECT count(*) FROM location_records WHERE merchant_id='target'",
                    &[]
                )?
                .get::<_, i64>(0),
            2
        );
        Ok(())
    })?;
    let mut missing = outlet;
    missing.merchant = crate::location::MerchantReference::Source {
        source: "missing".into(),
        external_id: "missing".into(),
    };
    assert!(upgraded.import_locations(&[missing]).is_err());
    Ok(())
}

#[test]
fn version_thirteen_views_are_removed_without_changing_catalog_responses() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
    let lease = PostgresStore::temporary(&base)?;
    let source: SourceRecord = serde_json::from_value(serde_json::json!({
        "source":"fixture", "external_id":"original-input",
        "merchant":{"id":"input-id","name":"Café 東京","website":"https://cafe.test",
            "logo_url":"https://cafe.test/logo.png","logo_source":"Fixture",
            "markets":["CA"],"aliases":["BANK CAFE"],"sources":["https://cafe.test/source"]},
        "attribution":"Fixture credit", "license":"Test licence", "url":"https://fixture.test",
        "version":"v1", "raw":{"countryHints":["CA","US"],"negativeAliases":["UNRELATED"],
            "transaction_text_regexp":"^CAFE", "parent_id":"parent","discarded":"payload"}
    }))?;
    let mut manual = source.merchant.clone();
    manual.id = "manual".into();
    lease.put(&manual)?;
    let empty: Merchant =
        serde_json::from_value(serde_json::json!({"id":"empty","name":"Empty fields"}))?;
    lease.put(&empty)?;
    lease.import(std::slice::from_ref(&source))?;
    lease.link("fixture", "original-input", "manual")?;
    let location: LocationRecord = serde_json::from_value(serde_json::json!({
        "source":"fixture","external_id":"outlet",
        "merchant":{"source":"fixture","external_id":"original-input"},
        "location":{"name":"Café branch","precision":"outlet","address":"1 Main St",
            "city":"Montréal","region":"QC","postal_code":"H1A","country":"CA",
            "store_number":"001","latitude":45.5,"longitude":-73.6},
        "aliases":["CAFE 001"],"place_ids":{"google":"google-place"},
        "transaction_pattern":"^CAFE 001", "manual_override":true,
        "attribution":"Fixture credit", "license":"Test licence", "url":"https://fixture.test"
    }))?;
    lease.import_locations(&[location])?;
    for (description, location) in [
        (
            "Opaque café",
            serde_json::json!({"city":"Montréal","country":"CA"}),
        ),
        ("Absent geography", serde_json::Value::Null),
    ] {
        let request = serde_json::from_value(serde_json::json!({
            "description":description,"country":"CA","amount":"12.34","currency":"CAD",
            "location":location,"extra":{"nested":{"items":[1,null,"東京"]}}
        }))?;
        let mut resolution = crate::resolution::Resolution::supported(
            &request,
            manual.clone(),
            vec![source.clone()],
        );
        resolution.verified = true;
        resolution.evidence = Some("Reviewed fixture".into());
        lease.save_resolution(&resolution)?;
    }
    let (expected, resolutions, exports, revision) = lease.run(|client| {
        let mut expected = postgres_dedupe_snapshot(client)?;
        client.batch_execute(include_str!("version_14_json.sql"))?;
        client.batch_execute(include_str!("version_13_documents.sql"))?;
        client.batch_execute("GRANT SELECT ON merchants_documents,manual_merchants_documents,source_records_documents,location_records_documents,descriptor_resolution_documents TO PUBLIC")?;
        fn merchants(client: &mut Client, table: &str) -> Result<Vec<Merchant>> {
            client.query(&format!("SELECT data FROM {table} ORDER BY id"), &[])?.iter()
                .map(|r| Ok(serde_json::from_str(r.get(0))?)).collect()
        }
        expected.merchants = merchants(client, "merchants_documents")?;
        expected.manual = merchants(client, "manual_merchants_documents")?;
        expected.sources = client.query("SELECT merchant_id,data FROM source_records_documents ORDER BY source,external_id", &[])?.iter()
            .map(|r| Ok((r.get(0), serde_json::from_str(r.get(1))?))).collect::<Result<_>>()?;
        expected.locations = client.query("SELECT data FROM location_records_documents ORDER BY id", &[])?.iter()
            .map(|r| Ok(serde_json::from_str(r.get(0))?)).collect::<Result<_>>()?;
        let resolutions = client.query("SELECT data FROM descriptor_resolution_documents ORDER BY id", &[])?.iter()
            .map(|r| Ok(serde_json::from_str::<crate::resolution::Resolution>(r.get(0))?)).collect::<Result<Vec<_>>>()?;
        let revision: i64 = client.query_one("SELECT revision FROM catalog_revision", &[])?.get(0);
        let exports = client.query("SELECT merchant_id,data FROM source_records_documents WHERE source='fixture' ORDER BY external_id", &[])?.iter()
            .map(|r| Ok(serde_json::json!({"merchant_id":r.get::<_,String>(0), "record":serde_json::from_str::<serde_json::Value>(r.get(1))?})))
            .collect::<Result<Vec<_>>>()?;
        Ok((expected, resolutions, exports, revision))
    })?;
    let upgraded = PostgresStore::connect(lease.temporary_url().unwrap(), true)?;
    let actual = upgraded.dedupe_snapshot()?;
    let mut expected = expected;
    // Physical source backfills advance the catalog revision; evidence and IDs
    // must remain identical. Monotonic revision is checked separately below.
    expected.revision = actual.revision;
    assert_eq!(
        serde_json::to_value(actual)?,
        serde_json::to_value(expected)?
    );
    assert_eq!(
        serde_json::to_value(upgraded.resolutions(None, 100)?)?,
        serde_json::to_value(resolutions)?
    );
    assert_eq!(upgraded.source_records("fixture", None, 100, 0)?, exports);
    upgraded.run(move |client| {
        assert!(client.query_one("SELECT revision FROM catalog_revision", &[])?.get::<_, i64>(0) >= revision);
        assert_eq!(client.query_one("SELECT count(*) FROM pg_class WHERE relnamespace='public'::regnamespace AND relkind='v' AND relname<>'source_identities'", &[])?.get::<_, i64>(0), 0);
        for table in ["merchants","manual_merchants","source_records","location_records","descriptor_resolutions","source_provenance"] {
            assert!(client.query_one("SELECT EXISTS(SELECT 1 FROM information_schema.role_table_grants WHERE table_schema='public' AND table_name=$1 AND grantee='PUBLIC' AND privilege_type='SELECT')", &[&table])?.get::<_, bool>(0));
        }
        client.execute("UPDATE merchants SET website='https://updated.test' WHERE id='manual'", &[])?;
        Ok(())
    })?;
    assert_eq!(
        upgraded.get("manual")?.unwrap().website.as_deref(),
        Some("https://updated.test")
    );
    assert!(
        upgraded
            .resolutions(None, 100)?
            .iter()
            .all(|r| r.merchant.website.as_deref() == Some("https://updated.test"))
    );
    // Reinitialization cannot recreate the projections.
    PostgresStore::connect(lease.temporary_url().unwrap(), true)?.run(|client| {
        assert_eq!(client.query_one("SELECT count(*) FROM pg_class WHERE relnamespace='public'::regnamespace AND relkind='v' AND relname<>'source_identities'", &[])?.get::<_, i64>(0), 0);
        Ok(())
    })
}

#[test]
fn schema_upgrade_waits_for_readers_without_extending_runtime_timeouts() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
    let lease = PostgresStore::temporary(&base)?;
    lease.run(|client| {
        client.batch_execute(include_str!("version_14_json.sql"))?;
        client.batch_execute(include_str!("version_13_documents.sql"))?;
        Ok(())
    })?;
    let url = lease.temporary_url().unwrap().to_owned();
    let blocker_url = url.clone();
    let (ready, acquired) = std::sync::mpsc::channel();
    let blocker = std::thread::spawn(move || -> Result<()> {
        let tls = MakeTlsConnector::new(native_tls::TlsConnector::builder().build()?);
        let mut client = Client::connect(&blocker_url, tls)?;
        let mut tx = client.transaction()?;
        tx.batch_execute("LOCK TABLE merchants_documents IN ACCESS SHARE MODE")?;
        ready.send(())?;
        std::thread::sleep(Duration::from_secs(17));
        tx.commit()?;
        Ok(())
    });
    acquired.recv()?;
    let upgraded = PostgresStore::connect(&url, true);
    blocker.join().expect("reader thread panicked")?;
    upgraded?.run(|client| {
        assert_eq!(
            client
                .query_one("SELECT version FROM ultrafinance_schema", &[])?
                .get::<_, i32>(0),
            18
        );
        assert_eq!(
            client
                .query_one("SHOW lock_timeout", &[])?
                .get::<_, String>(0),
            "10s"
        );
        assert_eq!(
            client
                .query_one("SHOW statement_timeout", &[])?
                .get::<_, String>(0),
            "15s"
        );
        Ok(())
    })
}

#[test]
fn document_view_migration_preserves_custom_dependencies_on_failure() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
    let lease = PostgresStore::temporary(&base)?;
    lease.run(|client| {
        client.batch_execute(include_str!("version_14_json.sql"))?;
        client.batch_execute(include_str!("version_13_documents.sql"))?;
        client.batch_execute("CREATE VIEW custom_export AS SELECT data FROM merchants_documents; GRANT SELECT ON merchants_documents TO PUBLIC")?;
        Ok(())
    })?;
    assert!(PostgresStore::connect(lease.temporary_url().unwrap(), true).is_err());
    lease.run(|client| {
        assert_eq!(client.query_one("SELECT version FROM ultrafinance_schema", &[])?.get::<_, i32>(0), 13);
        assert!(client.query_one("SELECT to_regclass('custom_export') IS NOT NULL AND to_regclass('descriptor_resolution_documents') IS NOT NULL", &[])?.get::<_, bool>(0));
        assert!(!client.query_one("SELECT EXISTS(SELECT 1 FROM information_schema.role_table_grants WHERE table_schema='public' AND table_name='merchants' AND grantee='PUBLIC')", &[])?.get::<_, bool>(0));
        client.batch_execute("DROP VIEW custom_export")?;
        Ok(())
    })?;
    PostgresStore::connect(lease.temporary_url().unwrap(), true)?;
    Ok(())
}

#[test]
#[cfg(unix)]
fn file_import_reconnects_after_idle_timeout_during_slow_counting() -> Result<()> {
    use std::io::Write;

    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    let first_pid: i32 = store.run(|client| {
        client.batch_execute("SET idle_session_timeout = '1s'")?;
        Ok(client.query_one("SELECT pg_backend_pid()", &[])?.get(0))
    })?;
    let directory = std::env::temp_dir().join(format!("ultra-slow-count-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory)?;
    let path = directory.join("knowledge.json");
    let created = std::process::Command::new("mkfifo").arg(&path).status()?;
    assert!(created.success(), "could not create slow input pipe");
    let records = crate::import::merchant_studio(
        r#"{"schemaVersion":"1.0","merchants":[{"id":"slow-count","canonicalName":"Slow Count Fixture","website":"https://slow-count.test"}]}"#,
    )?;
    let payload = serde_json::to_vec(&records)?;
    let writer_path = path.clone();
    let writer = std::thread::spawn(move || -> Result<()> {
        let mut pipe = std::fs::OpenOptions::new().write(true).open(&writer_path)?;
        pipe.write_all(b"[")?;
        // The importer is now counting, with no SQL activity. Let the server
        // expire the original connection before releasing the remaining JSON.
        std::thread::sleep(Duration::from_secs(2));
        // Keep the first reader on the pipe, but supply a stable regular file
        // for the subsequent import pass. Both contain exactly the same JSON.
        std::fs::remove_file(&writer_path)?;
        std::fs::write(&writer_path, &payload)?;
        pipe.write_all(&payload[1..])?;
        Ok(())
    });
    let result = store.reconcile_file(path, None, false, 100);
    let written = writer.join().expect("slow input writer panicked");
    std::fs::remove_dir_all(directory)?;
    written?;
    let (report, available, selected) = result?;
    assert_eq!((available, selected), (1, 1));
    assert_eq!(report.delta.added, 1);
    let second_pid: i32 =
        store.run(|client| Ok(client.query_one("SELECT pg_backend_pid()", &[])?.get(0)))?;
    assert_ne!(
        first_pid, second_pid,
        "import should use a fresh backend after counting"
    );
    assert!(
        store
            .resolve_source("merchant-studio", "slow-count")?
            .is_some()
    );
    Ok(())
}

#[test]
fn version_fourteen_json_lists_migrate_without_changing_api_values() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
    let lease = PostgresStore::temporary(&base)?;
    let merchant: Merchant = serde_json::from_value(serde_json::json!({
        "id":"typed-fixture","name":"Café 東京","markets":["CA","JP"],
        "aliases":["quoted \"name\"", "back\\slash", "東京"],
        "sources":["https://example.test/é"]
    }))?;
    lease.put(&merchant)?;
    let expected = lease.get(&merchant.id)?;
    lease.run(|client| {
        client.batch_execute(include_str!("version_14_json.sql"))?;
        // Historic nullable lists migrate to empty native arrays.
        client.batch_execute("INSERT INTO merchants(id,name,markets_json,aliases_json,sources_json) VALUES('nullable','No lists',NULL,NULL,NULL)")?;
        client.batch_execute("CREATE VIEW custom_lists AS SELECT aliases_json FROM merchants")?;
        Ok(())
    })?;
    assert!(PostgresStore::connect(lease.temporary_url().unwrap(), true).is_err());
    lease.run(|client| {
        assert_eq!(client.query_one("SELECT version FROM ultrafinance_schema", &[])?.get::<_, i32>(0), 14);
        assert_eq!(client.query_one("SELECT pg_typeof(markets_json)::text FROM merchants LIMIT 1", &[])?.get::<_, String>(0), "text");
        assert!(client.query_one("SELECT indisvalid FROM pg_index WHERE indexrelid='merchants_markets'::regclass", &[])?.get::<_, bool>(0));
        client.batch_execute("DROP VIEW custom_lists")?;
        Ok(())
    })?;
    let migrated = PostgresStore::connect(lease.temporary_url().unwrap(), true)?;
    assert_eq!(
        serde_json::to_value(migrated.get(&merchant.id)?)?,
        serde_json::to_value(expected)?
    );
    assert!(migrated.get("nullable")?.unwrap().markets.is_empty());
    migrated.run(|client| {
        assert_eq!(client.query_one("SELECT pg_typeof(markets)::text FROM merchants LIMIT 1", &[])?.get::<_, String>(0), "text[]");
        assert_eq!(client.query_one("SELECT count(*) FROM information_schema.columns WHERE table_schema='public' AND table_name IN ('merchants','manual_merchants','location_records') AND column_name LIKE '%\\_json' ESCAPE '\\'", &[])?.get::<_, i64>(0), 0);
        assert!(client.query_one("SELECT indisvalid FROM pg_index WHERE indexrelid='merchants_markets'::regclass", &[])?.get::<_, bool>(0));
        Ok(())
    })?;
    assert_eq!(migrated.list(Some("JP"), 10, 0)?.total, 1);
    PostgresStore::connect(lease.temporary_url().unwrap(), true)?;
    Ok(())
}

#[test]
fn version_fifteen_removes_unused_indexes_without_changing_evidence() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
    let lease = PostgresStore::temporary(&base)?;
    let url = lease.temporary_url().unwrap().to_owned();
    let records = crate::import::merchant_studio(
        r#"{"schemaVersion":"1.0","merchants":[{"id":"alpha","canonicalName":"Alpha","website":"https://alpha.test"}]}"#,
    )?;
    lease.import(&records)?;
    let id = lease.resolve_source("merchant-studio", "alpha")?;
    let revision: i64 = lease.run(|client| {
        client.batch_execute(include_str!("version_16_source_metadata.sql"))?;
        client.batch_execute("CREATE INDEX merchant_search_trigrams ON merchant_search USING GIN(text gin_trgm_ops); CREATE INDEX merchant_identity_name ON merchant_identity_keys(normalized_name); UPDATE ultrafinance_schema SET version=15")?;
        Ok(client.query_one("SELECT revision FROM catalog_revision", &[])?.get(0))
    })?;
    assert!(PostgresStore::connect(&url, false).is_err());
    let upgraded = PostgresStore::connect(&url, true)?;
    assert_eq!(upgraded.resolve_source("merchant-studio", "alpha")?, id);
    upgraded.run(move |client| {
        assert!(client.query_one("SELECT revision FROM catalog_revision", &[])?.get::<_, i64>(0) >= revision);
        assert!(client.query_one("SELECT to_regclass('merchant_search_trigrams') IS NULL AND to_regclass('merchant_identity_name') IS NULL", &[])?.get::<_, bool>(0));
        assert_eq!(client.query_one("SELECT count(*) FROM merchants", &[])?.get::<_, i64>(0), 1);
        assert!(client.query_one("SELECT tokens @@ plainto_tsquery('simple','alpha') FROM merchant_search", &[])?.get::<_, bool>(0));
        Ok(())
    })?;
    PostgresStore::connect(&url, true)?;
    Ok(())
}

#[test]
fn shared_source_provenance_migrates_variants_and_refreshes_independently() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
    let lease = PostgresStore::temporary(&base)?;
    let mut records = crate::import::merchant_studio(
        r#"{"schemaVersion":"1.0","merchants":[{"id":"one","canonicalName":"One"},{"id":"two","canonicalName":"Two"},{"id":"three","canonicalName":"Three"}]}"#,
    )?;
    records[2].attribution = "Café 東京's dataset".into();
    records[2].license = "Separate licence".into();
    records[2].url = "https://example.test/second".into();
    lease.import(&records)?;
    let before = lease.source_records("merchant-studio", None, 10, 0)?;
    lease.run(|client| {
        assert_eq!(
            client
                .query_one(
                    "SELECT count(DISTINCT source_provenance_id) FROM source_records",
                    &[]
                )?
                .get::<_, i64>(0),
            2
        );
        client.batch_execute(include_str!("version_16_source_metadata.sql"))?;
        client.batch_execute("DELETE FROM source_provenance; GRANT SELECT ON source_records TO PUBLIC; REVOKE ALL ON source_provenance FROM PUBLIC; CREATE VIEW custom_source_urls AS SELECT url FROM source_records")?;
        Ok(())
    })?;
    let url = lease.temporary_url().unwrap();
    assert!(PostgresStore::connect(url, true).is_err());
    lease.run(|client| {
        assert_eq!(
            client
                .query_one("SELECT version FROM ultrafinance_schema", &[])?
                .get::<_, i32>(0),
            16
        );
        assert_eq!(
            client
                .query_one("SELECT count(*) FROM custom_source_urls", &[])?
                .get::<_, i64>(0),
            3
        );
        client.batch_execute("DROP VIEW custom_source_urls")?;
        Ok(())
    })?;
    let upgraded = PostgresStore::connect(url, true)?;
    assert_eq!(
        upgraded.source_records("merchant-studio", None, 10, 0)?,
        before
    );
    records[0].license = "Updated licence for only one record".into();
    let delta = upgraded.import(&records)?;
    assert_eq!(delta.updated, 1);
    assert_eq!(delta.unchanged, 2);
    let refreshed = upgraded.source_records("merchant-studio", None, 10, 0)?;
    for previous in before {
        let current = refreshed
            .iter()
            .find(|r| r["record"]["external_id"] == previous["record"]["external_id"])
            .unwrap();
        if previous["record"]["external_id"] == records[0].external_id {
            assert_eq!(current["record"]["license"], records[0].license);
        } else {
            assert_eq!(*current, previous);
        }
    }
    upgraded.run(|client| {
        assert!(client.query_one("SELECT EXISTS(SELECT 1 FROM information_schema.role_table_grants WHERE table_schema='public' AND table_name='source_provenance' AND grantee='PUBLIC' AND privilege_type='SELECT')", &[])?.get::<_, bool>(0));
        assert_eq!(client.query_one("SELECT count(DISTINCT source_provenance_id) FROM source_records", &[])?.get::<_, i64>(0), 3);
        assert_eq!(client.query_one("SELECT count(*) FROM information_schema.columns WHERE table_schema='public' AND table_name='source_records' AND column_name IN ('attribution','license','url')", &[])?.get::<_, i64>(0), 0);
        let mut tx = client.transaction()?;
        let other: i64 = tx.query_one("INSERT INTO source_provenance(source,attribution,license,url) VALUES('other','a','b','c') RETURNING id", &[])?.get(0);
        assert!(tx.execute("UPDATE source_records SET source_provenance_id=$1", &[&other]).is_err());
        tx.rollback()?;
        Ok(())
    })?;
    Ok(())
}
