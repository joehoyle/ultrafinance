use anyhow::Result;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::{HeaderValue, StatusCode, header},
    middleware,
    response::{Html, IntoResponse, Response},
    routing::get,
};
use serde::Serialize;
use std::{env, sync::Arc, time::Duration};
use ultrafinance_core::{EnrichRequest, EnrichResponse, Enricher};
use utoipa::{OpenApi, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};
use utoipa_scalar::{Scalar, Servable};

#[derive(OpenApi)]
#[openapi(
    info(title = "Ultrafinance API", description = "Synchronous merchant enrichment. Requests return a completed result, with no background jobs. This API currently requires no client authentication."),
    tags((name = "Enrichment", description = "Resolve bank descriptions against the merchant catalog"), (name = "Health", description = "Service liveness"))
)]
struct ApiDoc;

#[derive(Serialize, ToSchema)]
struct HealthResponse {
    /// Service liveness indicator. Does not check the database or upstream provider.
    #[schema(example = "ok")]
    status: &'static str,
}

#[derive(Serialize, ToSchema)]
struct ApiError {
    error: ErrorDetail,
}

#[derive(Serialize, ToSchema)]
struct ErrorDetail {
    /// invalid_request, enrichment_failed, or enrichment_timeout.
    code: String,
    message: String,
}

fn api_router() -> (Router<Arc<Enricher>>, utoipa::openapi::OpenApi) {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(health))
        .routes(routes!(enrich))
        .layer(middleware::map_response(no_index))
        .split_for_parts()
}

fn router(enricher: Enricher) -> Router {
    let (api, spec) = api_router();
    let mut site = Router::new();
    for (path, content_type, content) in [
        (
            "/robots.txt",
            "text/plain; charset=utf-8",
            include_bytes!("../../../website/robots.txt").as_slice(),
        ),
        (
            "/sitemap.xml",
            "application/xml; charset=utf-8",
            include_bytes!("../../../website/sitemap.xml").as_slice(),
        ),
        (
            "/assets/share.png",
            "image/png",
            include_bytes!("../../../website/assets/share.png").as_slice(),
        ),
        (
            "/assets/favicon.svg",
            "image/svg+xml",
            include_bytes!("../../../website/assets/favicon.svg").as_slice(),
        ),
        (
            "/assets/favicon.ico",
            "image/x-icon",
            include_bytes!("../../../website/assets/favicon.ico").as_slice(),
        ),
        (
            "/favicon.ico",
            "image/x-icon",
            include_bytes!("../../../website/assets/favicon.ico").as_slice(),
        ),
        (
            "/assets/apple-touch-icon.png",
            "image/png",
            include_bytes!("../../../website/assets/apple-touch-icon.png").as_slice(),
        ),
        (
            "/assets/icon-512.png",
            "image/png",
            include_bytes!("../../../website/assets/icon-512.png").as_slice(),
        ),
    ] {
        site = site.route(
            path,
            get(move || async move { site_asset(content_type, content) }),
        );
    }
    site.route(
        "/",
        get(|| async { Html(include_str!("../../../website/index.html")) }),
    )
    .merge(api)
    .merge(Scalar::with_url("/docs", spec.clone()).title("Ultrafinance API documentation"))
    .route("/openapi.json", get(move || async move { Json(spec) }))
    .layer(DefaultBodyLimit::max(64 * 1024))
    .with_state(Arc::new(enricher))
}

fn site_asset(content_type: &'static str, content: &'static [u8]) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        content,
    )
        .into_response()
}

async fn no_index(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert("x-robots-tag", HeaderValue::from_static("noindex"));
    response
}

/// Check whether the HTTP service is running.
#[utoipa::path(get, path = "/health", tag = "Health", responses(
    (status = 200, description = "Service is running", body = HealthResponse, example = json!({"status":"ok"}))
))]
async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

/// Enrich a transaction with a merchant from the catalog.
///
/// Only description is required; unknown top-level fields are rejected. The JSON body
/// is limited to 64 KiB. A unique verified exact alias can resolve locally; fuzzy or
/// ambiguous candidates require provider evaluation. No candidates or insufficient
/// evidence returns unresolved with null data, not an HTTP error. Transaction fields
/// and candidate records are sent to TypeSafe when evaluation runs. The overall
/// enrichment deadline is 25 seconds. Configuration, database and provider failures
/// return 502 rather than unresolved.
#[utoipa::path(post, path = "/v1/enrich", tag = "Enrichment",
    request_body(content = EnrichRequest, example = json!({"description":"LS","amount":"142.97","currency":"CAD","country":"CA","extra":{"bank_category":["Food and Drink","Restaurants"]}})),
    responses(
        (status = 200, description = "Completed enrichment; matched or unresolved", body = EnrichResponse,
            examples(
                ("unresolved" = (value = json!({"merchant":{"status":"unresolved","data":null}}))),
                ("matched" = (value = json!({"merchant":{"status":"matched","data":{"id":"mer_example","name":"Example Café","country":"CA"}}})))
            )),
        (status = 400, description = "Malformed JSON", body = ApiError),
        (status = 413, description = "Request body exceeds 64 KiB", body = ApiError),
        (status = 415, description = "Missing or unsupported JSON content type", body = ApiError),
        (status = 422, description = "Invalid fields, unknown fields, or failed validation", body = ApiError,
            example = json!({"error":{"code":"invalid_request","message":"country must be a two-letter uppercase code"}})),
        (status = 502, description = "Enrichment failed, including configuration or provider errors", body = ApiError,
            example = json!({"error":{"code":"enrichment_failed","message":"Merchant evaluation failed"}})),
        (status = 504, description = "Enrichment exceeded the 25-second deadline", body = ApiError,
            example = json!({"error":{"code":"enrichment_timeout","message":"Merchant evaluation timed out"}}))
    )
)]
async fn enrich(
    State(enricher): State<Arc<Enricher>>,
    payload: Result<Json<EnrichRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(request) => request,
        Err(error) => return api_error(error.status(), "invalid_request", &error.body_text()),
    };
    if let Err(error) = request.validate() {
        return api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            &error.to_string(),
        );
    }
    match tokio::time::timeout(Duration::from_secs(25), enricher.enrich(&request)).await {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(error)) => {
            eprintln!("Enrichment failed: {error:#}");
            api_error(
                StatusCode::BAD_GATEWAY,
                "enrichment_failed",
                "Merchant evaluation failed",
            )
        }
        Err(_) => api_error(
            StatusCode::GATEWAY_TIMEOUT,
            "enrichment_timeout",
            "Merchant evaluation timed out",
        ),
    }
}

fn api_error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(ApiError {
            error: ErrorDetail {
                code: code.into(),
                message: message.into(),
            },
        }),
    )
        .into_response()
}

#[tokio::main]
async fn main() -> Result<()> {
    if env::args().nth(1).as_deref() == Some("--print-openapi") {
        println!("{}", api_router().1.to_pretty_json()?);
        return Ok(());
    }
    let path = env::var_os("ULTRAFINANCE_MERCHANTS").map(std::path::PathBuf::from);
    let store = if let Some(path) = path {
        let store = ultrafinance_core::store::MerchantStore::memory()?;
        for merchant in ultrafinance_core::load_catalog(Some(&path))? {
            store.put(&merchant)?;
        }
        store
    } else {
        let database = env::var_os("ULTRAFINANCE_DB")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "data/ultrafinance.sqlite".into());
        ultrafinance_core::store::MerchantStore::open(&database)?
    };
    let threshold = env::var("ULTRAFINANCE_MATCH_THRESHOLD")
        .unwrap_or_else(|_| "0.95".into())
        .parse()?;
    let enricher = Enricher::with_store(
        env::var("TYPESAFE_API_KEY").ok(),
        env::var("JEV_MODEL").unwrap_or_else(|_| "jev-latest".into()),
        threshold,
        store,
    )?;
    let address = env::var("ULTRAFINANCE_BIND").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    eprintln!(
        "Ultrafinance listening at http://{}",
        listener.local_addr()?
    );
    axum::serve(listener, router(enricher))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use serde_json::Value;
    use tower::ServiceExt;
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
        let reviewed: Value = serde_json::from_str(include_str!("../openapi.json")).unwrap();
        assert_eq!(
            spec, reviewed,
            "API contract changed; review and regenerate the OpenAPI snapshot (see README)"
        );
        assert_eq!(spec["paths"].as_object().unwrap().len(), 2);
        assert!(spec["paths"]["/health"]["get"].is_object());
        assert!(spec["paths"]["/v1/enrich"]["post"].is_object());
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
                serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap())
                    .unwrap();
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
        for path in ["/", "/health"] {
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
                serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap())
                    .unwrap();
            if expected == StatusCode::OK {
                assert_eq!(value["merchant"]["status"], "unresolved");
                assert!(value["merchant"]["data"].is_null());
            } else {
                assert_eq!(value["error"]["code"], "invalid_request");
            }
        }
    }
}
