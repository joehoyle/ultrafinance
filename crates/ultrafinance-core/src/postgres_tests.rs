use super::*;

fn merchant(id: &str, name: &str) -> Merchant {
    Merchant {
        id: id.into(),
        name: name.into(),
        country: Some("CA".into()),
        website: None,
        logo_url: Some("https://example.com/logo.png".into()),
        logo_source: Some("manual".into()),
        aliases: vec![],
        sources: vec![],
    }
}
fn record(id: &str, name: &str) -> SourceRecord {
    SourceRecord {
        source: "test".into(),
        external_id: id.into(),
        merchant: merchant(id, name),
        attribution: "Test".into(),
        license: "test".into(),
        url: "https://example.com".into(),
        version: Some("1".into()),
        raw: serde_json::json!({}),
    }
}

/// Run explicitly against an empty, disposable PostgreSQL database. CI supplies
/// this database as a service; never point this test at the production catalog.
#[tokio::test]
#[ignore = "requires ULTRAFINANCE_TEST_DATABASE_URL pointing at an empty disposable PostgreSQL database"]
async fn postgres_migration_imports_search_and_concurrency() -> Result<()> {
    let url = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")?;
    let pg = MerchantStore::initialize_postgres(&url)?;
    MerchantStore::initialize_postgres(&url)?; // migrations are repeatable
    let log_id = uuid::Uuid::new_v4().to_string();
    pg.write_log(
        &log_id,
        "test-batch",
        "started",
        None,
        &serde_json::json!({"request":{"description":"test"}}),
    )?;
    assert!(
        pg.enrichment_logs(Some("started"), None, 10, 0)?
            .iter()
            .any(|r| r["id"] == log_id && r["finished_at"].is_null())
    );
    pg.write_log(
        &log_id,
        "test-batch",
        "matched",
        Some("snapshot-merchant"),
        &serde_json::json!({"response":{"merchant":{"data":{"id":"snapshot-merchant"}}}}),
    )?;
    let logs = pg.enrichment_logs(Some("matched"), Some("snapshot-merchant"), 10, 0)?;
    assert_eq!(logs.len(), 1);
    assert!(logs[0]["finished_at"].is_string());

    let directory =
        std::env::temp_dir().join(format!("ultrafinance-migration-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("catalog.sqlite");
    let sqlite = MerchantStore::open(&path)?;
    let a = record("external-a", "Julius Café");
    let b = record("external-b", "Beta merchant");
    sqlite.import(&[a.clone(), b.clone()])?;
    let id = sqlite.resolve_source("test", "external-a")?.unwrap();
    let mut manual = merchant(&id, "Julius Café corrected");
    manual.aliases = vec!["VERIFIED ALIAS".into()];
    sqlite.put(&manual)?;
    sqlite.link("test", "external-b", &id)?;
    let mut outlets: Vec<crate::location::LocationRecord> = serde_json::from_str(include_str!(
        "../../../data/locations/open-enrichment-au.json"
    ))?;
    for outlet in &mut outlets {
        outlet.merchant = crate::location::MerchantReference::Source {
            source: "test".into(),
            external_id: "external-a".into(),
        };
    }
    sqlite.import_locations(&outlets)?;
    let original_locations = sqlite.locations(&id)?;
    let original = sqlite.fingerprint()?;
    // A bad derived SQLite row must roll back the entire destination, rather
    // than silently changing the catalog during migration.
    let raw = Connection::open(&path)?;
    let data: String = raw.query_row("SELECT data FROM merchants WHERE id=?1", [&id], |r| {
        r.get(0)
    })?;
    let mut corrupted: Merchant = serde_json::from_str(&data)?;
    corrupted.name = "Corrupted derived record".into();
    raw.execute(
        "UPDATE merchants SET data=?2 WHERE id=?1",
        params![id, serde_json::to_string(&corrupted)?],
    )?;
    assert!(pg.migrate_sqlite(&path).is_err());
    assert_eq!(pg.list(None, 10, 0)?.total, 0);
    raw.execute(
        "UPDATE merchants SET data=?2 WHERE id=?1",
        params![id, data],
    )?;
    drop(raw);
    assert_eq!(pg.migrate_sqlite(&path)?, 1);
    assert_eq!(pg.fingerprint()?, original);
    assert_eq!(
        serde_json::to_value(pg.locations(&id)?)?,
        serde_json::to_value(&original_locations)?
    );
    assert_eq!(sqlite.fingerprint()?, original);
    assert!(pg.migrate_sqlite(&path).is_err());
    assert_eq!(pg.fingerprint()?, original);
    assert_eq!(pg.resolve_source("test", "external-a")?, Some(id.clone()));
    assert_eq!(pg.resolve_source("test", "external-b")?, Some(id.clone()));
    pg.import_locations(&outlets)?;
    assert_eq!(
        serde_json::to_value(pg.locations(&id)?)?,
        serde_json::to_value(&original_locations)?
    );
    let mut corrected = outlets[0].clone();
    corrected.manual_override = true;
    corrected.location.name = Some("Corrected outlet".into());
    pg.import_locations(&[corrected])?;
    pg.import_locations(&outlets)?;
    assert!(
        pg.locations(&id)?
            .iter()
            .any(|r| r.location.name.as_deref() == Some("Corrected outlet"))
    );
    let verified = pg.search("verified alias", Some("CA"), 10)?;
    assert!(verified[0].trusted);
    assert_eq!(verified[0].merchant.logo_url, manual.logo_url);
    let imported = pg.search("Beta merchant", None, 10)?;
    assert!(!imported[0].trusted);
    assert_eq!(imported[0].provenance.len(), 2);
    let mut refresh = a;
    refresh.merchant.name = "Bad refresh name".into();
    refresh.merchant.aliases = vec!["NEW IMPORTED ALIAS".into()];
    pg.import(&[refresh.clone(), b])?;
    assert_eq!(
        pg.search("new imported alias", None, 10)?[0].merchant.name,
        manual.name
    );
    assert_eq!(pg.resolve_source("test", "external-b")?, Some(id.clone()));
    let before = pg.fingerprint()?;
    let mut bad = record("bad", " ");
    bad.merchant.id.clear();
    assert!(pg.import(&[record("new", "Brand new"), bad]).is_err());
    assert!(pg.import(&[refresh.clone(), refresh]).is_err());
    assert_eq!(before, pg.fingerprint()?);
    assert!(pg.link("test", "external-a", "missing").is_err());
    assert_eq!(before, pg.fingerprint()?);

    // Updates become visible through independently connected API/import clients.
    let second = MerchantStore::postgres(&url)?;
    let mut cafe = merchant("manual-cafe", "Julius Café");
    cafe.aliases = vec!["OLD DESCRIPTION".into()];
    pg.put(&cafe)?;
    assert!(
        second
            .search("JULIUS CAFE", Some("CA"), 10)?
            .iter()
            .any(|c| c.merchant.id == "manual-cafe" && c.trusted)
    );
    assert!(
        second
            .search("Julus cafe", Some("CA"), 10)?
            .iter()
            .any(|c| c.merchant.id == "manual-cafe")
    );
    assert!(
        second
            .search("Juli", Some("CA"), 10)?
            .iter()
            .any(|c| c.merchant.id == "manual-cafe")
    );
    assert!(second.search("LS", None, 10)?.is_empty());
    cafe.aliases = vec!["NEW DESCRIPTION".into()];
    pg.put(&cafe)?;
    assert!(
        !second
            .search("OLD DESCRIPTION", None, 10)?
            .iter()
            .any(|c| c.exact)
    );
    assert!(second.search("NEW DESCRIPTION", None, 10)?[0].trusted);
    second.search("\" OR * (NEAR) --", None, 10)?;
    let mut us = merchant("us", "Julius Café");
    us.country = Some("US".into());
    pg.put(&us)?;
    assert!(
        !second
            .search("Julius cafe", Some("CA"), 10)?
            .iter()
            .any(|c| c.merchant.id == "us")
    );
    let mut negative = record("amazon", "Amazon");
    negative.merchant.aliases = vec!["Amazon web services".into()];
    negative.raw = serde_json::json!({"negativeAliases":["amazon web services"]});
    pg.import(&[negative])?;
    assert!(pg.search("Amazon web services", None, 10)?.is_empty());
    assert_eq!(second.list(Some("US"), 50, 0)?.total, 1);
    assert_eq!(second.list(None, 1, 0)?.merchants.len(), 1);
    assert!(second.list(None, 0, 0).is_err());
    assert!(second.list(Some("ca"), 10, 0).is_err());

    // Two concurrent importers must retain the same source ID, without orphan rows.
    let concurrent = record("race", "Concurrent merchant");
    let first = pg.clone();
    let other = second.clone();
    let copy = concurrent.clone();
    let t1 = std::thread::spawn(move || first.import(&[copy]));
    let t2 = std::thread::spawn(move || other.import(&[concurrent]));
    t1.join().unwrap()?;
    t2.join().unwrap()?;
    assert_eq!(
        pg.resolve_source("test", "race")?,
        second.resolve_source("test", "race")?
    );
    assert_eq!(
        pg.search("Concurrent merchant", None, 10)?
            .iter()
            .filter(|c| c.exact)
            .count(),
        1
    );

    // Preserve exact collisions even if the caller requests a one-item shortlist.
    for n in 0..3 {
        pg.put(&merchant(&format!("collision-{n}"), "Collision"))?;
    }
    assert_eq!(
        pg.search("Collision", None, 1)?
            .iter()
            .filter(|c| c.exact)
            .count(),
        3
    );
    // Drop worker clients from an async runtime safely.
    super::tests::regex_search_scenarios(&pg)?;
    drop(second);
    drop(pg);
    drop(sqlite);
    std::fs::remove_dir_all(directory)?;
    Ok(())
}
