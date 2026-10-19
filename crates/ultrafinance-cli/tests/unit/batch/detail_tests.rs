use super::*;
#[test]
fn jev_scores_and_abstention_are_distinct_from_search_similarity() {
    let candidates = vec![json!({"merchant":{"name":"Amazon", "id":"a"}, "score":1.0})];
    let answer = json!({"choice":"candidate_0", "confidence":0.9, "probabilities":{"candidate_0":0.8, "none":0.2}});
    let table = candidate_table(&candidates, &answer, None);
    assert!(table.contains("Search similarity"));
    assert!(table.contains("1.000"));
    assert!(table.contains("80.0%"));
    assert!(table.contains("Jev choice"));
    assert!(!table.contains("MATCHED"));
    let summary = jev_summary(&answer, &candidates, &json!(0.95));
    assert!(summary.contains("confidence (derived diagnostic) 90.0%"));
    assert!(summary.contains("below match threshold"));
    let none = json!({"choice":"none", "confidence":0.99, "probabilities":{"none":0.98}});
    assert!(jev_summary(&none, &candidates, &json!(0.95)).contains("Jev selected no merchant"));
    let boundary = json!({"choice":"candidate_0", "confidence":0.8,
        "probabilities":{"candidate_0":0.9,"none":0.1}});
    assert!(!jev_summary(&boundary, &candidates, &json!(0.9)).contains("below match threshold"));
}
#[test]
fn discovery_choices_use_their_own_shortlist_and_null_context_is_omitted() {
    let result = eval::CaseResult {
        id: "case".into(),
        description: "BANK ORIGINAL".into(),
        request: serde_json::from_value(json!({"description":"BANK ORIGINAL"})).unwrap(),
        candidates: vec![],
        search_trace: Default::default(),
        expected: eval::Expected::Unlabeled,
        expected_local_id: None,
        candidate_ids: vec![],
        expected_rank: None,
        predicted_id: Some("discovered".into()),
        matched: Some(true),
        correct: None,
        error: None,
        latency_ms: 1.0,
        failure: None,
        enrichment: Some(json!({"method":"discovery", "threshold":0.95,
            "candidates":[{"merchant":{"id":"discovered","name":"New Shop"},"score":0.7}],
            "provider_answer":{"choice":"candidate_0", "confidence":0.99, "probabilities":{"candidate_0":0.98}}})),
    };
    let rendered = render_case(9, &result, Mode::Enrich).unwrap();
    assert!(rendered.contains("10. case"));
    assert!(rendered.contains("Jev choice: New Shop"));
    assert!(rendered.contains("Outcome: Matched: New Shop [discovered]"));
    assert!(!rendered.contains("null"));
    assert!(!rendered.contains("Context:"));
    assert!(
        render_case(9, &result, Mode::Search)
            .unwrap()
            .contains("Jev: not evaluated (search mode)")
    );
}
