use super::*;
use serde_json::json;

#[test]
fn captured_response_is_embedded_verbatim_as_safe_text() {
    let example = Example {
        endpoint: "https://example.com/v1/enrich".into(),
        captured_at: "2026-10-17T12:00:00Z".into(),
        request: json!({"description":"JOE'S CAFE","country":"CA"}),
        response: json!({"merchant":{"data":{"name":"</code><script>alert(1)</script>{{EXAMPLE_CAPTURED_AT}}"}},"attributions":["Source"]}),
    };
    let html = render(
        "{{EXAMPLE_API_REQUEST}}\n{{EXAMPLE_CLI_REQUEST}}\n{{EXAMPLE_RESPONSE}}\n{{EXAMPLE_CAPTURED_AT}}",
        &example,
    );
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(html.contains("{{EXAMPLE_CAPTURED_AT}}"));
    assert!(html.contains("Source"));
    assert!(html.ends_with("2026-10-17T12:00:00Z"));
    assert_eq!(shell_quote("JOE'S CAFE"), "'JOE'\\''S CAFE'");
}

#[test]
fn packaged_homepage_renders_the_saved_request_and_full_response() {
    let saved: Example =
        serde_json::from_str(include_str!("../../../../../website/example.json")).unwrap();
    let html = homepage();
    assert!(!html.contains("{{EXAMPLE_"));
    assert!(html.contains(&highlight_json(&display_json(&saved.response, 0))));
    assert!(html.contains(&escape(saved.request["description"].as_str().unwrap())));
    assert!(html.contains(&saved.captured_at));
    assert!(!html.contains("NORTHSTAR"));
}

#[test]
fn compact_json_preserves_payload_and_puts_merchant_before_attributions() {
    let value = json!({
        "attributions":["A long source with a URL: https://example.com/credits"],
        "location":{"status":"unresolved","data":null},
        "merchant":{"status":"matched","data":{"id":"brand","name":"Café \"Brand\"","markets":["CA","US"]}}
    });
    let formatted = display_json(&value, 0);
    assert_eq!(serde_json::from_str::<Value>(&formatted).unwrap(), value);
    assert!(formatted.find("\"merchant\"").unwrap() < formatted.find("\"attributions\"").unwrap());
    assert!(formatted.contains("[\"CA\", \"US\"]"));
}

#[tokio::test]
async fn homepage_works_with_an_unreachable_database() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    use ultrafinance_core::{Enricher, store::MerchantStore};

    let store =
        MerchantStore::postgres_lazy("postgresql://nobody@127.0.0.1:1/unavailable?sslmode=disable")
            .unwrap();
    let app = crate::router(Enricher::with_store(None, "jev-latest".into(), 0.95, store).unwrap());
    let response = app
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), homepage().as_bytes());
}
