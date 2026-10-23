use super::*;
use serde_json::json;

#[test]
fn nested_results_hide_metadata_without_changing_input_evidence_or_internal_values() {
    let original = json!({
        "results":[{"data":{"merchant":{"data":{"id":"brand","name":"Brand","sources":["private"],"logo_source":"private","logo_url":"/logo.webp"}},"attributions":["private"]}}],
        "candidates":[{"merchant":{"name":"Brand"},"provenance":[{"source":"private","license":"private"}],"score":0.9}],
        "outlet":{"source":"private","external_id":"private","attribution":"private","license":"private","url":"private","location":{"id":"loc_1","city":"Toronto"}},
        "request":{"extra":{"sources":["user evidence"],"provenance":"user evidence"}}
    });
    let public = value(&original).unwrap();
    assert!(public["results"][0]["data"].get("attributions").is_none());
    assert!(
        public["results"][0]["data"]["merchant"]["data"]
            .get("sources")
            .is_none()
    );
    assert!(
        public["results"][0]["data"]["merchant"]["data"]
            .get("logo_source")
            .is_none()
    );
    assert_eq!(
        public["results"][0]["data"]["merchant"]["data"]["logo_url"],
        "/logo.webp"
    );
    assert!(public["candidates"][0].get("provenance").is_none());
    assert_eq!(public["candidates"][0]["score"], 0.9);
    assert_eq!(
        public["outlet"],
        json!({"location":{"id":"loc_1","city":"Toronto"}})
    );
    assert_eq!(public["request"], original["request"]);
    assert!(original["candidates"][0].get("provenance").is_some());
}
