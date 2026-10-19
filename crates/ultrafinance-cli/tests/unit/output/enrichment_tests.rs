use super::*;

#[test]
fn selected_name_lists_each_catalog_record_without_claiming_a_match() {
    let choice = serde_json::json!({"records":[
        {"id":"rona-a", "name":"Rona", "website":"https://rona.ca"},
        {"id":"rona-b", "name":"Rona", "website":null},
        {"id":"rona-c", "name":"Rona", "website":"https://other.example/\u{1b}"}
    ]});
    let rendered = selected_catalog_records(&choice);
    for expected in [
        "3 catalog records for the selected business",
        "rona-a",
        "rona-b",
        "rona-c",
        "https://rona.ca",
        "https://other.example/",
        "Merchant ID",
        "Website",
    ] {
        assert!(rendered.contains(expected), "missing {expected}");
    }
    assert!(!rendered.contains('\u{1b}'));
    assert!(
        selected_catalog_records(&serde_json::json!({"records":[{"id":"single"}]}))
            .contains("single")
    );
}

#[test]
fn kind_abstention_is_not_a_failed_merchant_match() {
    let answer = serde_json::json!({"choice":"unknown","confidence":0.74,
        "probabilities":{"unknown":0.77,"purchase":0.20,"transfer":0.03}});
    let rendered = kind_answer(&answer, Some(0.9));
    assert!(rendered.contains("Kind"));
    assert!(rendered.contains("model abstained"));
    assert!(!rendered.contains("Merchant"));
    assert!(!rendered.contains("Below"));
}

#[test]
fn weak_kind_selection_explains_classification_threshold() {
    let answer = serde_json::json!({"choice":"purchase","confidence":0.95,
        "probabilities":{"purchase":0.87,"unknown":0.13}});
    let rendered = kind_answer(&answer, Some(0.9));
    assert!(rendered.contains("Below classification threshold"));
    assert!(rendered.contains("Kind outcome: unknown"));
    assert!(!rendered.contains("model abstained"));
    assert!(!rendered.contains("match threshold"));
    let confident = serde_json::json!({"choice":"purchase","confidence":0.8,
        "probabilities":{"purchase":0.90,"unknown":0.10}});
    let rendered = kind_answer(&confident, Some(0.9));
    assert!(!rendered.contains("Below"));
    assert!(!rendered.contains("Kind outcome: unknown"));
    assert!(rendered.contains("derived diagnostic"));
}

#[test]
fn trading_name_abstention_explains_why_identity_was_not_evaluated() {
    let answer = serde_json::json!({"choice":"candidate_0", "confidence":0.99,
        "probabilities":{"candidate_0":0.89,"none":0.11}});
    let candidates = vec![serde_json::json!({"merchant":{"name":"Alpha Cafe"}})];
    let rendered = name_answer(&answer, Some(&candidates), Some(0.9));
    assert!(rendered.contains("Trading name recognition"));
    assert!(rendered.contains("Below name threshold"));
    assert!(rendered.contains("catalog identity resolution skipped"));
    let answer =
        serde_json::json!({"choice":"none", "confidence":1.0,"probabilities":{"none":1.0}});
    assert!(
        name_answer(&answer, Some(&candidates), Some(0.9)).contains("No trading name selected")
    );
}

#[test]
fn detail_stages_separate_kind_name_and_deterministic_identity() {
    let details = serde_json::json!({
        "interpretation":{"original":"THAI EXPRESS","hypotheses":[{"merchant_text":"THAI EXPRESS"}]},
        "threshold":0.75,"model":"fixture","method":"provider","status":"matched",
        "response":{"merchant":{"status":"matched","data":{"name":"Thai Express"}},"kind":"purchase","location":{"status":"unresolved"}},
        "provider_choices":[{"merchant":{"name":"Thai Express"},"records":[{"id":"mer_ca","name":"Thai Express","website":"https://thaiexpress.ca","markets":["CA"]}]}],
        "provider_answer":{"choice":"candidate_0","identity_method":"deterministic","decision_stage":"identity","identity_choices":["candidate_0"],"identity_country":"CA",
            "name_answer":{"choice":"candidate_0","confidence":0.98,"probabilities":{"candidate_0":0.99,"none":0.01}},
            "kind":{"choice":"purchase","confidence":0.99,"probabilities":{"purchase":0.99,"unknown":0.01}}},
        "provider_requests":[{"body":{"secret_marker":"do not render"}}]
    });
    let rendered = render_enrichment_details(&details).unwrap();
    let headings = [
        "1. Identify payment processor",
        "2. Prepare merchant descriptor",
        "3. Interpret merchant descriptor",
        "4. Retrieve merchant candidates",
        "5. Classify transaction kind",
        "6. Select merchant name",
        "7. Resolve catalog identity",
        "8. Fallbacks",
        "9. Resolve location independently",
    ];
    let positions: Vec<_> = headings
        .iter()
        .map(|heading| rendered.find(heading).unwrap())
        .collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(rendered.starts_with("Enrichment details: THAI EXPRESS\nMerchant: Thai Express"));
    assert!(rendered.contains("No second Jev call"));
    assert_eq!(rendered.matches("mer_ca").count(), 1);
    assert!(!rendered.contains("secret_marker"));
    assert!(!rendered.contains("100.0%"));
}

#[test]
fn detail_name_failure_still_shows_independent_kind_and_skipped_identity() {
    let details = serde_json::json!({"threshold":0.75,"provider_answer":{
        "decision_stage":"name","name_answer":{"choice":"none","probabilities":{"none":1.0}},
        "kind":{"choice":"purchase","probabilities":{"purchase":0.98}}},
        "response":{"kind":"purchase","merchant":{"status":"unresolved"},"location":{"status":"unresolved"}}});
    let rendered = render_enrichment_details(&details).unwrap();
    assert!(rendered.contains("Outcome: purchase"));
    assert!(rendered.contains("Skipped: name selection"));
    assert!(!rendered.contains("Identity selection"));
    assert!(rendered.contains("no merchant outlet selected"));
}

#[test]
fn processor_details_distinguish_preferred_choice_from_accepted_match() {
    let details = serde_json::json!({
        "threshold":0.75,
        "processor_resolution":{"status":"unresolved", "provider_answer":{"choice":"candidate_0", "probabilities":{"candidate_0":0.62,"none":0.38}}},
        "merchant_descriptor":{"original":"SumUp STARBUCKS", "text":"SumUp STARBUCKS", "status":"unchanged"}
    });
    let rendered = render_enrichment_details(&details).unwrap();
    assert!(rendered.contains("Jev preferred: candidate_0 · 62.0%"));
    assert!(
        rendered.contains(
            "Not accepted: 62.0% is below the 75.0% threshold. Processor text is retained."
        )
    );
    assert!(rendered.contains("Merchant lookup: SumUp STARBUCKS"));
    assert!(!rendered.contains("Removed confirmed processor span:"));
}
