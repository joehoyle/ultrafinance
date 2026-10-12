use super::*;
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use tower::ServiceExt;
#[tokio::test]
async fn enrichment_responses_hide_aliases_but_still_match_them() {
    let merchants = serde_json::from_value(json!([
        {"id":"beta","name":"Beta Shop","markets":["US"],
         "website":"https://beta.example","aliases":["BETA BILL"],
         "sources":["https://source.example"]}
    ]))
    .unwrap();
    let app = router(Enricher::new(None, "jev-latest".into(), 0.95, merchants).unwrap());
    for (url, payload) in [
        ("/v1/enrich", json!({"description":"BETA BILL"})),
        (
            "/v1/enrich/batch",
            json!({"transactions":[{"description":"BETA BILL"}]}),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::post(url)
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{url}");
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        let result = if url.ends_with("/batch") {
            assert_eq!(body["results"][0]["status"], "success");
            &body["results"][0]["data"]
        } else {
            &body
        };
        assert_eq!(result["merchant"]["status"], "matched");
        assert_eq!(result["merchant"]["data"]["id"], "beta");
        assert_eq!(result["merchant"]["data"]["markets"], json!(["US"]));
        assert_eq!(
            result["merchant"]["data"]["website"],
            "https://beta.example"
        );
        assert!(result["merchant"]["data"].get("aliases").is_none());
        assert!(result["merchant"]["data"].get("sources").is_none());
    }
}

#[test]
fn public_enrichment_keeps_attributions_without_merchant_sources() {
    let internal: ultrafinance_core::EnrichResponse = serde_json::from_value(json!({
        "merchant": {"status":"matched", "data": {
            "id":"beta", "name":"Beta Shop", "sources":["https://source.example"]
        }},
        "location": {"status":"unresolved", "data":null},
        "attributions": ["Example source (CC-BY-4.0, https://source.example)"]
    }))
    .unwrap();
    let public = serde_json::to_value(response::EnrichResponse::from(internal)).unwrap();
    assert!(public["merchant"]["data"].get("sources").is_none());
    assert_eq!(
        public["attributions"],
        json!(["Example source (CC-BY-4.0, https://source.example)"])
    );
}

#[tokio::test]
async fn bulk_results_preserve_order_and_errors_and_use_their_own_body_limit() {
    let merchants =
        serde_json::from_value(json!([{"id":"alpha","name":"Alpha Cafe","markets":["CA"]}]))
            .unwrap();
    let app = router(Enricher::new(None, "jev-latest".into(), 0.95, merchants).unwrap());
    let payload = json!({"transactions":[
        {"description":"Alpha Cafe"},
        {"description":""},
        {"description":"Alpha Cafe PURCHASE"},
        {"description":"NO KNOWN MERCHANT ZZZZZZ", "extra":{"notes":"x".repeat(70 * 1024)}}
    ]});
    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/enrich/batch")
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["results"].as_array().unwrap().len(), 4);
    assert_eq!(
        body["results"][0]["data"]["merchant"]["data"]["id"],
        "alpha"
    );
    assert_eq!(body["results"][1]["code"], "invalid_request");
    assert_eq!(body["results"][2]["code"], "enrichment_failed");
    assert!(!body.to_string().contains("TYPESAFE_API_KEY"));
    for (payload, status) in [
        (
            json!({"transactions":[]}).to_string(),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"transactions":vec![json!({"description":"test"});101]}).to_string(),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"transactions":[],"unknown":true}).to_string(),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (" ".repeat(1024 * 1024 + 1), StatusCode::PAYLOAD_TOO_LARGE),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::post("/v1/enrich/batch")
                    .header("content-type", "application/json")
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
}
#[tokio::test]
async fn catalog_browsing_search_and_validation_work_without_provider_calls() {
    let merchants = serde_json::from_value(json!([
        {"id":"alpha","name":"Alpha Cafe","markets":["CA"],"website":"https://alpha.example"},
        {"id":"beta","name":"Beta Shop","markets":["US"],"aliases":["BETA BILL"],"sources":["https://source.example"]},
        {"id":"gamma","name":"Gamma Market","markets":["CA"]}
    ]))
    .unwrap();
    let app = router(Enricher::new(None, "jev-latest".into(), 0.95, merchants).unwrap());
    for (url, total, ids) in [
        ("/v1/merchants?limit=2&offset=1", 3, vec!["beta", "gamma"]),
        ("/v1/merchants?market=CA", 2, vec!["alpha", "gamma"]),
        ("/v1/merchants?market=US", 1, vec!["beta"]),
        ("/v1/merchants?offset=99", 3, vec![]),
        ("/v1/merchants?q=BETA%20BILL", 1, vec!["beta"]),
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(url).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{url}");
        assert_eq!(response.headers()["x-robots-tag"], "noindex");
        let page: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(page["total"], total, "{url}");
        let found: Vec<_> = page["merchants"]
            .as_array()
            .unwrap()
            .iter()
            .map(|merchant| merchant["id"].as_str().unwrap())
            .collect();
        assert_eq!(found, ids, "{url}");
        assert!(
            page["merchants"]
                .as_array()
                .unwrap()
                .iter()
                .all(|merchant| merchant.get("provenance").is_none()
                    && merchant.get("aliases").is_none()
                    && merchant.get("sources").is_none()
                    && merchant.get("country").is_none()
                    && merchant.get("markets").is_some())
        );
    }
    for url in [
        "/v1/merchants?limit=0",
        "/v1/merchants?limit=101",
        "/v1/merchants?limit=no",
        "/v1/merchants?offset=-1",
        "/v1/merchants?offset=1000001",
        "/v1/merchants?market=ca",
        "/v1/merchants?country=CA",
        "/v1/merchants?unknown=yes",
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(url).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{url}");
        let error: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(error["error"]["code"], "invalid_request");
    }
    let empty = router(Enricher::new(None, "jev-latest".into(), 0.95, vec![]).unwrap());
    let response = empty
        .oneshot(Request::get("/v1/merchants").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let page: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(page["total"], 0);
    assert_eq!(page["merchants"], json!([]));
}
#[tokio::test]
async fn served_openapi_matches_reviewed_contract_and_docs_are_available() {
    let app = router(Enricher::new(None, "jev-latest".into(), 0.95, vec![]).unwrap());
    let response = app
        .clone()
        .oneshot(Request::get("/openapi.json").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let spec: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    let reviewed: Value = serde_json::from_str(include_str!("../../../openapi.json")).unwrap();
    assert_eq!(
        spec, reviewed,
        "API contract changed; review and regenerate the OpenAPI snapshot (see README)"
    );
    assert_eq!(spec["paths"].as_object().unwrap().len(), 4);
    assert!(spec["paths"]["/v1/enrich/batch"]["post"].is_object());
    assert!(spec["paths"]["/health"]["get"].is_object());
    assert!(spec["paths"]["/v1/enrich"]["post"].is_object());
    assert!(spec["paths"]["/v1/merchants"]["get"].is_object());
    assert!(
        spec["components"]["schemas"]["Merchant"]["properties"]
            .get("aliases")
            .is_none()
    );
    assert!(
        spec["components"]["schemas"]["Merchant"]["properties"]
            .get("sources")
            .is_none()
    );
    assert_eq!(
        spec["components"]["schemas"]["MerchantResult"]["oneOf"][1]["properties"]["data"]["type"],
        "null",
        "Unresolved results must describe null data, not an arbitrary value"
    );
    let response = app
        .oneshot(Request::get("/docs").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    let html = String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("Ultrafinance API documentation"));
    assert!(html.contains("@scalar/api-reference"));
    assert!(html.contains("/v1/enrich"));
}

#[tokio::test]
async fn documented_request_errors_match_http_behavior() {
    let app = router(Enricher::new(None, "jev-latest".into(), 0.95, vec![]).unwrap());
    let spec = serde_json::to_value(api_router().1).unwrap();
    for (content_type, body, expected) in [
        ("application/json", "{".into(), StatusCode::BAD_REQUEST),
        (
            "text/plain",
            "{}".into(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "application/json",
            r#"{"description":"LS","currency":"cad"}"#.into(),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "application/json",
            " ".repeat(64 * 1024 + 1),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::post("/v1/enrich")
                    .header("content-type", content_type)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(value["error"]["code"], "invalid_request");
        assert!(value["error"]["message"].is_string());
        assert_eq!(
            spec["paths"]["/v1/enrich"]["post"]["responses"][expected.as_str()]["content"]["application/json"]
                ["schema"]["$ref"],
            "#/components/schemas/ApiError"
        );
    }
}

#[tokio::test]
async fn crawler_and_share_assets_are_served_with_correct_types() {
    let app = router(Enricher::new(None, "jev-latest".into(), 0.95, vec![]).unwrap());
    for (path, content_type) in [
        ("/robots.txt", "text/plain; charset=utf-8"),
        ("/sitemap.xml", "application/xml; charset=utf-8"),
        ("/assets/share.png", "image/png"),
        ("/assets/favicon.svg", "image/svg+xml"),
        ("/assets/favicon.ico", "image/x-icon"),
        ("/favicon.ico", "image/x-icon"),
        ("/assets/apple-touch-icon.png", "image/png"),
        ("/assets/icon-512.png", "image/png"),
    ] {
        for method in ["GET", "HEAD"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{method} {path}");
            assert_eq!(response.headers()[header::CONTENT_TYPE], content_type);
            let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            if method == "HEAD" {
                assert!(bytes.is_empty());
            } else {
                assert!(!bytes.is_empty());
                if path == "/assets/share.png" {
                    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
                    assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 1200);
                    assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 630);
                }
            }
        }
    }
}
#[tokio::test]
async fn health_and_site_are_public() {
    let app = router(Enricher::new(None, "jev-latest".into(), 0.95, vec![]).unwrap());
    for path in ["/", "/sources", "/privacy", "/terms", "/health"] {
        let response = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
#[tokio::test]
async fn empty_catalog_and_validation_work_without_provider_calls() {
    let app = router(Enricher::new(None, "jev-latest".into(), 0.95, vec![]).unwrap());
    for (body, expected) in [
        (
            r#"{"description":"LS","extra":{"category":["Restaurants"]}}"#,
            StatusCode::OK,
        ),
        (r#"{"description":" "}"#, StatusCode::UNPROCESSABLE_ENTITY),
        (
            r#"{"description":"LS","contry":"CA"}"#,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::post("/v1/enrich")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(response.headers()["x-robots-tag"], "noindex");
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        if expected == StatusCode::OK {
            assert_eq!(value["merchant"]["status"], "unresolved");
            assert!(value["merchant"]["data"].is_null());
        } else {
            assert_eq!(value["error"]["code"], "invalid_request");
        }
    }
}
#[tokio::test]
async fn locations_are_independent_in_http_results_and_hints_are_validated() {
    let app = router(Enricher::new(None, "jev-latest".into(), 0.95, vec![]).unwrap());
    for (request, expected_status, expected_location) in [
        (
            json!({"description":"ZXQ #0006 HIALEAH FL"}),
            StatusCode::OK,
            Some("extracted"),
        ),
        (
            json!({"description":"UNKNOWN","location":{"city":"Bromont","country":"CA"}}),
            StatusCode::OK,
            Some("extracted"),
        ),
        (
            json!({"description":"UNKNOWN","country":"CA"}),
            StatusCode::OK,
            Some("unresolved"),
        ),
        (
            json!({"description":"UNKNOWN","location":{"country":"ca"}}),
            StatusCode::UNPROCESSABLE_ENTITY,
            None,
        ),
        (
            json!({"description":"UNKNOWN","location":{"city":" "}}),
            StatusCode::UNPROCESSABLE_ENTITY,
            None,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::post("/v1/enrich")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(request.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected_status);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        if let Some(status) = expected_location {
            assert_eq!(body["merchant"]["status"], "unresolved");
            assert_eq!(body["location"]["status"], status);
        }
    }
}
