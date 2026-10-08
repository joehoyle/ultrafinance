use anyhow::Result;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde_json::json;
use std::{env, sync::Arc, time::Duration};
use ultrafinance_core::{EnrichRequest, Enricher};

fn router(enricher: Enricher) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../../../website/index.html")) }),
        )
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/v1/enrich", post(enrich))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(Arc::new(enricher))
}

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
        Json(json!({"error":{"code":code,"message":message}})),
    )
        .into_response()
}

#[tokio::main]
async fn main() -> Result<()> {
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
