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
