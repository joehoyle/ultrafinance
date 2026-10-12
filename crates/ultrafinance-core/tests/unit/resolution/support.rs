pub(crate) fn exercise(store: &crate::store::MerchantStore) -> Result<()> {
    let merchant: Merchant = serde_json::from_value(
        json!({"id":"resolution-cafe","name":"Example Cafe","markets":["CA"]}),
    )?;
    store.put(&merchant)?;
    let request: EnrichRequest = serde_json::from_value(
        json!({"description":"opaque zxmq 123","country":"CA","amount":"10.00","extra":{"bank":"one"}}),
    )?;
    let context = context(&request);
    let mut resolution = Resolution {
        id: key(&context),
        context,
        merchant,
        provenance: vec![],
        verified: false,
        evidence: None,
    };
    assert!(store.resolutions(None, 10)?.is_empty());
    assert!(store.save_resolution(&resolution)?);
    assert_eq!(store.resolutions(Some(&resolution.id), 1)?.len(), 1);
    let candidates = store.search_request(&request, 10)?;
    assert_eq!(candidates[0].merchant.id, "resolution-cafe");
    assert!(!candidates[0].exact && !candidates[0].trusted);
    resolution.verified = true;
    assert!(store.save_resolution(&resolution).is_err());
    resolution.evidence = Some("Reviewed receipt and official website".into());
    assert!(store.save_resolution(&resolution)?);
    let candidates = store.search_request(&request, 10)?;
    assert!(candidates[0].exact && candidates[0].trusted);
    for changed in [
        json!({"country":"US"}),
        json!({"extra":{"bank":"two"}}),
        json!({"amount":"20.00"}),
    ] {
        let mut value = serde_json::to_value(&request)?;
        for (key, value2) in changed.as_object().unwrap() {
            value[key] = value2.clone();
        }
        let changed: EnrichRequest = serde_json::from_value(value)?;
        assert!(
            store
                .search_request(&changed, 10)?
                .iter()
                .all(|c| c.resolution_id.is_none())
        );
    }
    let mut old = resolution.clone();
    old.verified = false;
    old.evidence = None;
    assert!(store.save_resolution(&old).is_err());
    let merchant2: Merchant =
        serde_json::from_value(json!({"id":"resolution-other","name":"Different Shop"}))?;
    store.put(&merchant2)?;
    let mut conflict = resolution.clone();
    conflict.merchant = merchant2;
    assert!(store.save_resolution(&conflict).is_err());
    let records = crate::import::catalog(
        r#"[{"id":"duplicate","name":"Example Cafe"}]"#,
        "resolution-fixture",
    )?;
    store.import(&records)?;
    let retired = store
        .resolve_source("resolution-fixture", "duplicate")?
        .unwrap();
    let merge_request: EnrichRequest =
        serde_json::from_value(json!({"description":"merge fixture opaque"}))?;
    let old_mapping = Resolution::supported(&merge_request, store.get(&retired)?.unwrap(), vec![]);
    store.save_resolution(&old_mapping)?;
    let snapshot = store.dedupe_snapshot()?;
    store.apply_dedupe(&snapshot, &[vec!["resolution-cafe".into(), retired]])?;
    assert_eq!(
        store.resolutions(Some(&old_mapping.id), 1)?[0].merchant.id,
        "resolution-cafe"
    );
    // Saving an older caller's record follows the redirect into the current FK.
    store.save_resolution(&old_mapping)?;
    assert!(store.revoke_resolution(&resolution.id)?);
    assert!(store.resolutions(Some(&resolution.id), 1)?.is_empty());
    Ok(())
}
