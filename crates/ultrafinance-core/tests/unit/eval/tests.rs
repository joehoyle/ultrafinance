use super::*;

#[tokio::test]
async fn enrich_eval_retrieves_only_the_processor_cleaned_pool() -> Result<()> {
    let db = MerchantStore::temporary()?;
    let merchant = serde_json::from_value(serde_json::json!({"id":"verified-cafe","name":"Verified Cafe"}))?;
    db.put(&merchant)?;
    db.put(&serde_json::from_value(serde_json::json!({"id":"fixture-processor","name":"Fixture Pay","type":"payment_processor"}))?)?;
    db.save_processor_formats(vec![serde_json::from_value(serde_json::json!({
        "id":"fixture-prefix", "processor_id":"fixture-processor", "pattern":"FIX\\*(?P<merchant>.+)",
        "sources":["https://example.test/format"], "reviewed_on":"2026-10-19", "scope":"test fixture"
    }))?], false)?;
    let request = serde_json::from_value(serde_json::json!({"description":"Verified Cafe"}))?;
    let mut mapping = crate::resolution::Resolution::supported(&request, merchant, vec![]);
    mapping.verified = true;
    mapping.evidence = Some("Reviewed fixture".into());
    db.save_resolution(&mapping)?;
    // Only the cleaned, reviewed mapping can succeed: an unnecessary search of
    // the original processor-prefixed descriptor would access the missing table.
    let url = db.temporary_url().to_owned();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut client = postgres::Client::connect(&url, postgres_native_tls::MakeTlsConnector::new(native_tls::TlsConnector::builder().build()?))?;
        client.batch_execute("ALTER TABLE merchant_search RENAME TO unavailable_merchant_search")?;
        Ok(())
    }).await??;
    let suite = r#"{"version":1,"name":"processor-first","cases":[{"id":"wrapped","request":{"description":"FIX*Verified Cafe"},"expected":{"status":"matched","merchant":{"merchant_id":"verified-cafe"}}}]}"#;
    let report = run(suite, db, Mode::Enrich, None, "jev-latest".into(), 0.95).await?;
    let result = &report.results[0];
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(result.description, "FIX*Verified Cafe");
    assert_eq!(result.candidate_ids, ["verified-cafe"]);
    assert_eq!(result.expected_rank, Some(1));
    assert_eq!(result.correct, Some(true));
    assert!(result.candidates[0].resolution_id.is_some());
    assert_eq!(result.enrichment.as_ref().unwrap()["merchant_descriptor"]["text"], "Verified Cafe");
    Ok(())
}
#[tokio::test]
async fn offline_retrieval_and_exact_matching_are_separate_metrics() {
    let db = MerchantStore::temporary().unwrap();
    db.put(&crate::Merchant {
        counterparty_type: Default::default(),
        id: "mer_a".into(),
        name: "Julius Café".into(),
        markets: vec!["CA".into()],

        website: None,
        logo_url: None,
        logo_source: None,
        aliases: vec![],
        sources: vec![],
    })
    .unwrap();
    let cases = r#"{"version":1,"name":"test","cases":[{"id":"known","request":{"description":"Julius cafe"},"expected":{"status":"matched","merchant":{"merchant_id":"mer_a"}}},{"id":"unknown","request":{"description":"LS"},"expected":{"status":"unresolved"}},{"id":"missing","request":{"description":"Something else"},"expected":{"status":"matched","merchant":{"source":"test","external_id":"missing"}}}]}"#;
    let search = run(
        cases,
        db.clone(),
        Mode::Search,
        None,
        "jev-latest".into(),
        0.95,
    )
    .await
    .unwrap();
    assert_eq!(search.metrics.candidate_recall, Some(0.5));
    assert_eq!(search.metrics.accuracy, None);
    assert_eq!(search.metrics.missing_source_references, 1);
    let enrich = run(cases, db, Mode::Enrich, None, "jev-latest".into(), 0.95)
        .await
        .unwrap();
    assert_eq!(enrich.metrics.matches, Some(1));
    assert_eq!(enrich.metrics.match_precision, Some(1.0));
    assert_eq!(enrich.metrics.accuracy, Some(2.0 / 3.0));
    assert_eq!(enrich.metrics.merchant_recall, Some(0.5));
}
#[tokio::test]
async fn service_errors_cannot_be_counted_as_correct_unresolved() {
    let db = MerchantStore::temporary().unwrap();
    db.put(&crate::Merchant {
        counterparty_type: Default::default(),
        id: "mer_a".into(),
        name: "Julius Café".into(),
        markets: vec![],

        website: None,
        logo_url: None,
        logo_source: None,
        aliases: vec![],
        sources: vec![],
    })
    .unwrap();
    let suite = r#"{"version":1,"name":"errors","cases":[{"id":"fuzzy","request":{"description":"Julus cafe"},"expected":{"status":"unresolved"}}]}"#;
    let report = run(suite, db, Mode::Enrich, None, "jev-latest".into(), 0.95)
        .await
        .unwrap();
    assert_eq!(report.metrics.errors, 1);
    assert_eq!(report.metrics.accuracy, Some(0.0));
    assert_eq!(report.metrics.correct_unresolved, Some(0));
    assert_eq!(report.metrics.match_precision, None);
    assert!(
        report.results[0]
            .error
            .as_ref()
            .unwrap()
            .contains("not configured")
    );
    assert_eq!(report.results[0].correct, None);
}

#[tokio::test]
async fn unlabeled_outcomes_measure_coverage_without_claiming_accuracy() {
    let db = MerchantStore::temporary().unwrap();
    db.put(&crate::Merchant {
        counterparty_type: Default::default(),
        id: "a".into(),
        name: "Julius Cafe".into(),
        markets: vec![],

        website: None,
        logo_url: None,
        logo_source: None,
        aliases: vec![],
        sources: vec![],
    })
    .unwrap();
    let suite = r#"{"version":1,"name":"unlabeled","cases":[
        {"id":"match","request":{"description":"Julius Cafe"},"expected":{"status":"unlabeled"}},
        {"id":"none","request":{"description":"ZZQQXX"},"expected":{"status":"unlabeled"}},
        {"id":"error","request":{"description":"Julus cafe"},"expected":{"status":"unlabeled"}}
    ]}"#;
    let completed = std::sync::atomic::AtomicUsize::new(0);
    let report = run_with_progress(
        suite,
        db,
        Mode::Enrich,
        None,
        "jev-latest".into(),
        0.95,
        Some(&completed),
    )
    .await
    .unwrap();
    assert_eq!(completed.load(std::sync::atomic::Ordering::Relaxed), 3);
    assert_eq!(report.metrics.unlabeled, 3);
    assert_eq!(report.metrics.matches, Some(1));
    assert_eq!(report.metrics.unresolved, Some(1));
    assert_eq!(report.metrics.errors, 1);
    assert_eq!(report.metrics.match_rate, Some(1.0 / 3.0));
    assert_eq!(report.metrics.accuracy, None);
    assert_eq!(report.metrics.match_precision, None);
    assert_eq!(report.metrics.false_matches, None);
    assert_eq!(report.results[0].matched, Some(true));
    assert_eq!(report.results[1].matched, Some(false));
    assert_eq!(report.results[2].matched, None);
    assert!(report.results.iter().all(|r| r.correct.is_none()));
    let mixed = [
        CaseResult {
            id: "labeled".into(),
            request: serde_json::from_str(r#"{"description":"test"}"#).unwrap(),
            candidates: vec![],
            search_trace: Default::default(),
            description: "Labeled case".into(),
            expected: Expected::Unresolved,
            expected_local_id: None,
            candidate_ids: vec![],
            expected_rank: None,
            predicted_id: None,
            matched: Some(false),
            correct: Some(true),
            error: None,
            failure: None,
            enrichment: None,
            latency_ms: 1.0,
        },
        CaseResult {
            id: "unlabeled".into(),
            request: serde_json::from_str(r#"{"description":"test"}"#).unwrap(),
            candidates: vec![],
            search_trace: Default::default(),
            description: "Unlabeled case".into(),
            expected: Expected::Unlabeled,
            expected_local_id: None,
            candidate_ids: vec!["a".into()],
            expected_rank: None,
            predicted_id: Some("a".into()),
            matched: Some(true),
            correct: None,
            error: None,
            failure: None,
            enrichment: None,
            latency_ms: 1.0,
        },
    ];
    let metrics = summarize(&mixed, Mode::Enrich);
    assert_eq!(metrics.accuracy, Some(1.0));
    assert_eq!(metrics.match_precision, None);
    assert_eq!(metrics.false_matches, Some(0));
    assert_eq!(metrics.match_rate, Some(0.5));
}

#[test]
fn missing_labels_are_rejected_and_wrong_matches_are_penalized() {
    assert!(
        serde_json::from_str::<Suite>(
            r#"{"version":1,"name":"test","cases":[{"id":"x","request":{"description":"LS"}}]}"#
        )
        .is_err()
    );
    let wrong = CaseResult {
        id: "x".into(),
        request: serde_json::from_str(r#"{"description":"test"}"#).unwrap(),
        candidates: vec![],
        search_trace: Default::default(),
        description: "LS".into(),
        expected: Expected::Unresolved,
        expected_local_id: None,
        candidate_ids: vec!["a".into()],
        expected_rank: None,
        predicted_id: Some("a".into()),
        matched: Some(true),
        correct: Some(false),
        error: None,
        failure: None,
        enrichment: None,
        latency_ms: 1.0,
    };
    let metrics = summarize(&[wrong], Mode::Enrich);
    assert_eq!(metrics.false_matches, Some(1));
    assert_eq!(metrics.accuracy, Some(0.0));
    assert_eq!(metrics.match_precision, Some(0.0));
    assert_eq!(metrics.merchant_recall, None);
}

#[tokio::test]
async fn purchase_filter_keeps_unresolved_purchases_and_audits_exclusions() {
    let cases = serde_json::json!({"version":1,"name":"purchase-filter","cases":
        (0..6).map(|i| serde_json::json!({"id":i.to_string(),"request":{"description":"fixture"},"expected":{"status":"unlabeled"}})).collect::<Vec<_>>()});
    let mut report = run(
        &cases.to_string(),
        MerchantStore::temporary().unwrap(),
        Mode::Search,
        None,
        "test".into(),
        0.75,
    )
    .await
    .unwrap();
    report.purchases_only();
    assert_eq!(report.results.len(), 6);
    assert!(report.purchase_filter.is_none());
    report.mode = Mode::Enrich;
    for (result, kind) in report.results.iter_mut().zip([
        "purchase", "purchase", "transfer", "unknown", "purchase", "",
    ]) {
        result.enrichment = Some(serde_json::json!({"response":{"kind":kind}}));
        result.matched = Some(false);
    }
    report.results[0].predicted_id = Some("merchant".into());
    report.results[0].matched = Some(true);
    report.results[4].error = Some("provider failure".into());
    report.purchases_only();
    assert_eq!(report.metrics.cases, 2);
    assert_eq!(report.metrics.matches, Some(1));
    assert_eq!(report.metrics.unresolved, Some(1));
    assert_eq!(report.metrics.match_rate, Some(0.5));
    let filter = report.purchase_filter.as_ref().unwrap();
    assert_eq!(filter.total_cases, 6);
    assert_eq!(filter.excluded_results.len(), 4);
    assert_eq!(filter.excluded_errors, 1);
    report.purchases_only();
    assert_eq!(report.purchase_filter.as_ref().unwrap().total_cases, 6);
    report.purchase_filter = None;
    for result in &mut report.results {
        result.enrichment = None;
    }
    report.purchases_only();
    assert_eq!(report.metrics.cases, 0);
    assert_eq!(report.metrics.match_rate, None);
}

#[test]
fn matching_stages_and_funnel_distinguish_selection_from_identity() {
    let case = |stage: &str, choice: &str, probability: f64, ambiguous: bool| CaseResult {
        id: "fixture".into(),
        description: "Fixture".into(),
        request: serde_json::from_value(serde_json::json!({"description":"Fixture"})).unwrap(),
        candidates: vec![],
        search_trace: Default::default(),
        expected: Expected::Unlabeled,
        expected_local_id: None,
        candidate_ids: vec!["a".into()],
        expected_rank: None,
        predicted_id: None,
        matched: Some(false),
        correct: None,
        error: None,
        failure: None,
        enrichment: Some(serde_json::json!({"threshold":0.75,"provider_answer":{
            "decision_stage":stage,"choice":choice,"probabilities":{choice:probability},
            "catalog_resolution":if ambiguous {"ambiguous"} else {"resolved"}
        }})),
        latency_ms: 0.0,
    };
    let mut retrieval = case("name", "none", 1.0, false);
    retrieval.candidate_ids.clear();
    let low = case("name", "candidate_0", 0.74, false);
    let mut identity = case("identity", "none", 0.8, false);
    identity.enrichment.as_mut().unwrap()["provider_answer"]["name_answer"] =
        serde_json::json!({"choice":"candidate_0","probabilities":{"candidate_0":0.9}});
    let ambiguous = case("", "candidate_0", 0.9, true);
    let mut matched = case("identity", "candidate_0", 0.9, false);
    matched.predicted_id = Some("a".into());
    let mut error = case("", "none", 1.0, false);
    error.error = Some("provider failed".into());
    let cases = [retrieval, low, identity, ambiguous, matched, error];
    assert_eq!(
        cases[0].matching_failure(Mode::Enrich).unwrap().reason,
        "no_candidates"
    );
    assert_eq!(
        cases[1].matching_failure(Mode::Enrich).unwrap().reason,
        "below_threshold"
    );
    assert_eq!(
        cases[2].matching_failure(Mode::Enrich).unwrap().stage,
        "identity_resolution"
    );
    assert_eq!(
        cases[3].matching_failure(Mode::Enrich).unwrap().reason,
        "ambiguous_catalog_records"
    );
    assert!(cases[4].matching_failure(Mode::Enrich).is_none());
    assert_eq!(
        cases[5].matching_failure(Mode::Enrich).unwrap().stage,
        "enrichment"
    );
    let metrics = summarize(&cases, Mode::Enrich);
    assert_eq!(
        metrics
            .funnel
            .iter()
            .map(|step| step.remaining)
            .collect::<Vec<_>>(),
        vec![6, 5, 3, 1]
    );
    assert_eq!(
        metrics
            .funnel
            .iter()
            .map(|step| step.dropped)
            .sum::<usize>(),
        5
    );
    assert_eq!(metrics.failures_by_stage.values().sum::<usize>(), 5);
    assert_eq!(metrics.failures_by_stage["identity_resolution"], 2);
    assert_eq!(summarize(&cases, Mode::Search).funnel.len(), 2);
}
