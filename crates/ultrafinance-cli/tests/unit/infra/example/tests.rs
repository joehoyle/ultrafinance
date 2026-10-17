use super::*;
use serde_json::json;

#[test]
fn snapshot_preserves_actual_response_and_rejects_changed_matches() {
    let saved = json!({
        "request":{"description":"EXAMPLE"},
        "response":{"merchant":{"status":"matched","data":{"id":"expected"}}}
    });
    let response = json!({
        "merchant":{"status":"matched","data":{"id":"expected","name":"Example","logo_url":"https://example.com/logo.svg"}},
        "location":{"status":"unresolved","data":null},
        "attributions":["Actual source"]
    });
    let updated = capture(
        saved.clone(),
        response.clone(),
        "https://example.com/v1/enrich",
        "2026-10-17T12:00:00Z",
    )
    .unwrap();
    assert_eq!(updated["response"], response);
    assert_eq!(updated["request"], saved["request"]);
    assert_eq!(updated["captured_at"], "2026-10-17T12:00:00Z");
    for invalid in [
        json!({"merchant":{"status":"unresolved","data":null}}),
        json!({"merchant":{"status":"matched","data":{"id":"different","name":"Example"}}}),
        json!({"error":{"message":"Unavailable"}}),
    ] {
        assert!(
            capture(
                saved.clone(),
                invalid,
                "https://example.com/v1/enrich",
                "now"
            )
            .is_err()
        );
    }
}
