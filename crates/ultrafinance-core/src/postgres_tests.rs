use super::*;

#[test]
fn postgres_source_columns_retain_matching_fields_without_raw_payloads() -> Result<()> {
    let lease = MerchantStore::temporary()?;
    let mut client = postgres::Client::connect(
        lease.temporary_url(),
        postgres_native_tls::MakeTlsConnector::new(native_tls::TlsConnector::builder().build()?),
    )?;
    for (n, raw) in [
        serde_json::Value::Null,
        serde_json::json!("opaque source text"),
        serde_json::json!([1, "two"]),
        serde_json::json!(true),
        serde_json::json!({"transaction_text_regexp": true}),
        serde_json::json!({"transaction_text_regexp": "^CAFE", "parent_id": "parent"}),
    ]
    .into_iter()
    .enumerate()
    {
        let record: SourceRecord = serde_json::from_value(serde_json::json!({
            "source": "column-fixture", "external_id": n.to_string(),
            "merchant": {"id": n.to_string(), "name": format!("Fixture {n}")},
            "attribution": "Fixture", "license": "Test", "url": "https://example.com",
            "version": null, "raw": raw
        }))?;
        lease.import(std::slice::from_ref(&record))?;
        let row = client.query_one(
            "SELECT data,transaction_pattern,parent_id FROM source_records_documents WHERE source='column-fixture' AND external_id=$1",
            &[&n.to_string()],
        )?;
        let restored: SourceRecord = serde_json::from_str(row.get(0))?;
        assert_eq!(restored.raw, record.matching_raw());
        assert_eq!(
            row.get::<_, Option<String>>(1).as_deref(),
            raw.get("transaction_text_regexp").and_then(|v| v.as_str())
        );
        assert_eq!(
            row.get::<_, Option<String>>(2).as_deref(),
            raw.get("parent_id").and_then(|v| v.as_str())
        );
    }
    Ok(())
}

#[test]
fn postgres_schema_upgrade_is_atomic_and_preserves_reviewed_mappings() -> Result<()> {
    let lease = MerchantStore::temporary()?;
    exercise_market_migration(lease.temporary_url())
}

#[test]
fn postgres_clients_share_atomic_imports_and_exact_collisions() -> Result<()> {
    let lease = MerchantStore::temporary()?;
    let other = MerchantStore::postgres(lease.temporary_url())?;
    let mut records = crate::import::catalog(
        r#"[{"id":"one","name":"Alpha merchant"},{"id":"two","name":"Beta merchant"}]"#,
        "fixture",
    )?;
    lease.import(&records)?;
    let id = lease.resolve_source("fixture", "one")?.unwrap();
    records[0].merchant.name = "Refreshed Alpha".into();
    lease.import(&records)?;
    assert_eq!(other.resolve_source("fixture", "one")?, Some(id.clone()));
    assert_eq!(other.get(&id)?.unwrap().name, "Refreshed Alpha");
    let mut bad = records[0].clone();
    bad.merchant.name = " ".into();
    assert!(lease.import(&[records[1].clone(), bad]).is_err());
    let workers: Vec<_> = (0..4)
        .map(|n| {
            let store = lease.clone();
            std::thread::spawn(move || -> Result<()> {
                let merchant: Merchant = serde_json::from_value(
                    serde_json::json!({"id":format!("collision-{n}"),"name":"Collision"}),
                )?;
                store.put(&merchant)
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap()?;
    }
    assert_eq!(
        other
            .search("Collision", None, 1)?
            .iter()
            .filter(|c| c.exact)
            .count(),
        4
    );
    crate::resolution::exercise(&lease)?;
    Ok(())
}

fn exercise_market_migration(url: &str) -> Result<()> {
    use postgres::Client;
    use postgres_native_tls::MakeTlsConnector;
    let mut client = Client::connect(
        url,
        MakeTlsConnector::new(native_tls::TlsConnector::builder().build()?),
    )?;
    assert_eq!(
        client
            .query_one("SELECT COUNT(*) FROM merchants", &[])?
            .get::<_, i64>(0),
        0,
        "market migration fixture requires an empty database"
    );
    client.batch_execute("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")?;
    client.batch_execute(include_str!("../migrations/001_postgres.sql"))?;
    client.batch_execute(include_str!("../migrations/002_enrichment_log.sql"))?;
    client.batch_execute(include_str!("../migrations/003_locations.sql"))?;
    let old =
        serde_json::json!({"id":"pre-market","name":"Before markets","country":"CA"}).to_string();
    let source = serde_json::json!({
        "source":"pre-market-source","external_id":"external","merchant":{"id":"external","name":"Before markets","country":"ca"},
        "attribution":"Test","license":"Test","url":"https://example.com","version":null,"raw":{"countryHints":["US"]}
    }).to_string();
    client.batch_execute("UPDATE ultrafinance_schema SET version=2;")?;
    client.execute(
        "INSERT INTO merchants(id,country,data) VALUES('pre-market','CA',$1)",
        &[&old],
    )?;
    client.execute(
        "INSERT INTO manual_merchants(id,data) VALUES('pre-market',$1)",
        &[&old],
    )?;
    client.execute(
        "INSERT INTO source_records VALUES('pre-market-source','external','pre-market',$1)",
        &[&source],
    )?;
    client.batch_execute(crate::resolution::LEGACY_SCHEMA)?;
    let cached_request: crate::EnrichRequest = serde_json::from_value(
        serde_json::json!({"description":"legacy opaque reference","country":"CA"}),
    )?;
    let mut resolution = crate::resolution::Resolution::supported(
        &cached_request,
        serde_json::from_value(
            serde_json::json!({"id":"pre-market","name":"Before markets","markets":["CA"]}),
        )?,
        vec![],
    );
    resolution.verified = true;
    resolution.evidence = Some("Reviewed legacy fixture".into());
    client.execute(
        "INSERT INTO descriptor_resolutions(id,data) VALUES($1,$2)",
        &[&resolution.id, &serde_json::to_string(&resolution)?],
    )?;
    // Invalid source data rolls back earlier row conversions and schema changes.
    assert!(MerchantStore::initialize_postgres(url).is_err());
    assert_eq!(
        client
            .query_one("SELECT version FROM ultrafinance_schema", &[])?
            .get::<_, i32>(0),
        2
    );
    assert_eq!(
        client
            .query_one(
                "SELECT data FROM manual_merchants WHERE id='pre-market'",
                &[]
            )?
            .get::<_, String>(0),
        old
    );
    let mut repaired: Value = serde_json::from_str(&source)?;
    repaired["merchant"]["country"] = serde_json::json!("CA");
    client.execute(
        "UPDATE source_records SET data=$1 WHERE source='pre-market-source'",
        &[&repaired.to_string()],
    )?;
    assert!(MerchantStore::postgres(url).is_err());
    let store = MerchantStore::initialize_postgres(url)?;
    assert!(store.search_request(&cached_request, 10)?[0].trusted);
    assert_eq!(
        store.resolutions(Some(&resolution.id), 1)?[0].merchant.id,
        "pre-market"
    );
    let page = store.list(Some("US"), 10, 0)?;
    assert_eq!(page.total, 1);
    assert_eq!(page.merchants[0].markets, ["CA", "US"]);
    assert!(
        serde_json::to_value(&page.merchants[0])?
            .get("country")
            .is_none()
    );
    assert_eq!(
        store.resolve_source("pre-market-source", "external")?,
        Some("pre-market".into())
    );
    let fingerprint = store.fingerprint()?;
    assert_eq!(
        MerchantStore::initialize_postgres(url)?.fingerprint()?,
        fingerprint
    );
    assert_eq!(
        client
            .query_one("SELECT version FROM ultrafinance_schema", &[])?
            .get::<_, i32>(0),
        8
    );
    let has_country: bool = client.query_one("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_name='merchants' AND column_name='country')", &[])?.get(0);
    assert!(!has_country);
    store.revoke_resolution(&resolution.id)?;
    client.batch_execute("DELETE FROM source_records WHERE source='pre-market-source'; DELETE FROM manual_merchants WHERE id='pre-market'; DELETE FROM merchants WHERE id='pre-market';")?;
    Ok(())
}
