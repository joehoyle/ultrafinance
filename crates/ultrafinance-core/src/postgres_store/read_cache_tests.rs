use super::*;

#[test]
fn cached_reads_are_reused_and_follow_external_catalog_writes() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    let csv = "fsq_place_id,name,country,address,fsq_category_ids,date_closed\n1,Cafe,CA,1 Main St,\"[\"\"restaurant\"\"]\",\n";
    let mut place = crate::foursquare::prepare(csv, None, "ca")?.0.remove(0);
    store.import(std::slice::from_ref(&place))?;
    let id = store.resolve_source("foursquare", "place:1")?.unwrap();
    let first = store.cached_locations(&id)?;
    assert!(Arc::ptr_eq(&first, &store.cached_locations(&id)?));
    let other = PostgresStore::connect(store.temporary_url().unwrap(), false)?;
    place.raw["places"][0]["address"] = serde_json::json!("Updated Street");
    other.import(std::slice::from_ref(&place))?;
    let updated = store.cached_locations(&id)?;
    assert!(!Arc::ptr_eq(&first, &updated));
    assert_eq!(
        updated[0].location.address.as_deref(),
        Some("Updated Street")
    );

    let mut rule = place;
    rule.source = "open-enrichment".into();
    rule.external_id = "rule".into();
    rule.raw = serde_json::json!({"transaction_text_regexp":"^OPAQUE OLD"});
    other.import(std::slice::from_ref(&rule))?;
    assert!(
        store
            .search("OPAQUE OLD", None, 10)?
            .iter()
            .any(|c| c.regex_match_length.is_some())
    );
    let owner = store.0.clone();
    let (a, b) = store.run(move |client| {
        let mut tx = client.transaction()?;
        let mut cache = owner.reads.lock().unwrap();
        Ok((cache.rules(&mut tx)?, cache.rules(&mut tx)?))
    })?;
    assert!(Arc::ptr_eq(&a, &b));
    rule.raw["transaction_text_regexp"] = serde_json::json!("^OPAQUE NEW");
    other.import(&[rule])?;
    assert!(
        !store
            .search("OPAQUE OLD", None, 10)?
            .iter()
            .any(|c| c.regex_match_length.is_some())
    );
    assert!(
        store
            .search("OPAQUE NEW", None, 10)?
            .iter()
            .any(|c| c.regex_match_length.is_some())
    );
    let mut duplicate = updated[0].clone();
    duplicate.source = "directory".into();
    duplicate.external_id = "same-outlet".into();
    duplicate.location.id = Some("loc_duplicate".into());
    other.run(move |client| {
        // Seed two independent rows without the importer's automatic dedupe,
        // then exercise a redirect-only maintenance write on another connection.
        let (merchant, source, external) = location_reference(&duplicate.merchant);
        crate::columns::postgres_location_records(
            client,
            "loc_duplicate",
            &duplicate.source,
            &duplicate.external_id,
            &merchant,
            &source,
            &external,
            &serde_json::to_string(&duplicate)?,
        )?;
        Ok(())
    })?;
    let separate = store.cached_locations(&id)?;
    assert_eq!(separate.len(), 2);
    other.dedupe_locations(Some(&id), false)?;
    let consolidated = store.cached_locations(&id)?;
    assert_eq!(consolidated.len(), 1);
    assert!(!Arc::ptr_eq(&separate, &consolidated));
    Ok(())
}

#[test]
fn batch_log_writes_are_atomic_and_preserve_creation_times() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    let entry =
        |id: &str, status: &str| (id.into(), "batch".into(), status.into(), None, "{}".into());
    assert!(
        store
            .write_logs(vec![entry("one", "started"), entry("two", "invalid")])
            .is_err()
    );
    assert!(store.logs(None, None, 10, 0)?.is_empty());
    store.write_logs(vec![entry("one", "started"), entry("two", "started")])?;
    let before = store.logs(None, None, 10, 0)?;
    store.write_logs(vec![entry("one", "matched"), entry("two", "unresolved")])?;
    let after = store.logs(None, None, 10, 0)?;
    assert_eq!(after.len(), 2);
    for row in after {
        let old = before.iter().find(|r| r["id"] == row["id"]).unwrap();
        assert_eq!(row["created_at"], old["created_at"]);
        assert!(row["finished_at"].is_string());
        assert_ne!(row["status"], "started");
    }
    Ok(())
}

#[test]
fn search_vocabulary_backfills_and_follows_bulk_refreshes() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    let mut records = crate::import::catalog(
        r#"[{"id":"one","name":"Novabrand"},{"id":"two","name":"Secondbrand"}]"#,
        "fixture",
    )?;
    store.import(&records)?;
    // Recreate the actual previous schema: the next initialization must backfill
    // existing search rows, not just maintain future inserts.
    store.run(|client| {
        client.batch_execute("DROP TRIGGER merchant_search_words_insert ON merchant_search; DROP TRIGGER merchant_search_words_update ON merchant_search; DROP FUNCTION remember_merchant_search_words(); DROP TABLE merchant_search_words; UPDATE ultrafinance_schema SET version=10")?;
        Ok(())
    })?;
    let upgraded = PostgresStore::connect(store.temporary_url().unwrap(), true)?;
    upgraded.run(|client| {
        assert!(
            client
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM merchant_search_words WHERE word='novabrand')",
                    &[]
                )?
                .get::<_, bool>(0)
        );
        Ok(())
    })?;
    assert!(
        upgraded
            .search("Freshquorim", None, 10)?
            .iter()
            .all(|candidate| candidate.merchant.name != "Freshquorium")
    );
    records[0].merchant.name = "Freshquorium".into();
    upgraded.import(&records)?;
    upgraded.run(|client| {
        assert!(client.query_one("SELECT EXISTS(SELECT 1 FROM merchant_search_words WHERE word='freshquorium')", &[])?.get::<_,bool>(0));
        assert_eq!(client.query_one("SELECT count(*) FROM merchant_search WHERE tokens @@ plainto_tsquery('simple','novabrand')", &[])?.get::<_,i64>(0), 0);
        Ok(())
    })?;
    assert!(
        upgraded
            .search("Freshquorim", None, 10)?
            .iter()
            .any(|candidate| candidate.merchant.name == "Freshquorium")
    );
    Ok(())
}
