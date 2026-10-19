use super::*;
#[tokio::test]
async fn offline_retrieval_and_exact_matching_are_separate_metrics() {
    let db = MerchantStore::temporary().unwrap();
    db.put(&crate::Merchant {
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
        enrichment: None,
        latency_ms: 1.0,
    };
    let metrics = summarize(&[wrong], Mode::Enrich);
    assert_eq!(metrics.false_matches, Some(1));
    assert_eq!(metrics.accuracy, Some(0.0));
    assert_eq!(metrics.match_precision, Some(0.0));
    assert_eq!(metrics.merchant_recall, None);
}
