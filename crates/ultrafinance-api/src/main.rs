use anyhow::Result;
use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderValue, StatusCode, header},
    middleware,
    response::{Html, IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::{env, sync::Arc, time::Duration};
mod response;
mod website;

use response::{BatchEnrichResponse, BatchItemResult, EnrichResponse, MerchantPage};
use ultrafinance_core::batch::BatchEnrichRequest;
use ultrafinance_core::{EnrichRequest, Enricher};
use utoipa::{IntoParams, OpenApi, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};
use utoipa_scalar::{Scalar, Servable};

#[derive(OpenApi)]
#[openapi(
    info(title = "Ultrafinance API", description = "Synchronous merchant and location enrichment. Requests return a completed result, with no background jobs. This API currently requires no client authentication."),
    tags((name = "Enrichment", description = "Resolve merchants and transaction geography"), (name = "Merchants", description = "Browse and search the merchant catalog"), (name = "Health", description = "Service liveness"))
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
        .merge(
            OpenApiRouter::new()
                .routes(routes!(enrich_batch))
                .layer(DefaultBodyLimit::max(1024 * 1024)),
        )
        .routes(routes!(merchants))
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
    site.route("/", get(|| async { Html(website::homepage()) }))
        .route(
            "/sources",
            get(|| async { Html(include_str!("../../../website/sources.html")) }),
        )
        .route(
            "/privacy",
            get(|| async { Html(include_str!("../../../website/privacy.html")) }),
        )
        .route(
            "/terms",
            get(|| async { Html(include_str!("../../../website/terms.html")) }),
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

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
struct MerchantQuery {
    /// Optional name or alias search; at most 256 bytes. Searches return a bounded candidate pool.
    q: Option<String>,
    /// Filter by known operating market, as a two-letter uppercase country code. Coverage is not exhaustive.
    market: Option<String>,
    /// Page size, from 1 to 100. Defaults to 20.
    limit: Option<usize>,
    /// Number of results to skip, up to 1,000,000. Defaults to 0.
    offset: Option<usize>,
}

/// Explore the merchant catalog without calling the AI provider.
/// Browsing is alphabetical; search ranks a bounded candidate pool rather than
/// matching every catalog row. Search totals refer to that pool (at most 255 for
/// exact alias collisions, otherwise 100), not the full catalog. Results include
/// merchant records with known markets, without aliases or internal matching scores.
#[utoipa::path(get, path = "/v1/merchants", tag = "Merchants",
    params(MerchantQuery),
    responses(
        (status = 200, description = "Paginated catalog records or ranked search candidates", body = MerchantPage),
        (status = 400, description = "Invalid query parameters", body = ApiError),
        (status = 503, description = "Catalog unavailable or database schema incompatible", body = ApiError),
        (status = 504, description = "Catalog query timed out", body = ApiError)
    )
)]
async fn merchants(
    State(enricher): State<Arc<Enricher>>,
    query: Result<Query<MerchantQuery>, QueryRejection>,
) -> Response {
    let Query(query) = match query {
        Ok(query) => query,
        Err(error) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                &error.body_text(),
            );
        }
    };
    let q = query
        .q
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let market = query.market.filter(|value| !value.is_empty());
    let limit = query.limit.unwrap_or(20);
    let offset = query.offset.unwrap_or(0);
    if !(1..=100).contains(&limit)
        || offset > 1_000_000
        || q.as_ref().is_some_and(|value| value.len() > 256)
        || market
            .as_ref()
            .is_some_and(|value| value.len() != 2 || !value.bytes().all(|c| c.is_ascii_uppercase()))
    {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Use limit 1–100, offset 0–1000000, a search of at most 256 bytes, and a two-letter uppercase market code",
        );
    }
    match tokio::time::timeout(
        Duration::from_secs(50),
        enricher.list_merchants(q, market, limit, offset),
    )
    .await
    {
        Ok(Ok(page)) => Json(MerchantPage::from(page)).into_response(),
        Ok(Err(error)) => {
            eprintln!("Catalog query failed: {error:#}");
            if let Some(response) = schema_error(&error) {
                return response;
            }
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "catalog_unavailable",
                "The merchant catalog is unavailable. Please try again.",
            )
        }
        Err(_) => api_error(
            StatusCode::GATEWAY_TIMEOUT,
            "catalog_timeout",
            "The merchant catalog query timed out",
        ),
    }
}

/// Enrich a transaction with a merchant and independently supported location.
///
/// Only description is required; unknown top-level fields are rejected. The JSON body
/// is limited to 64 KiB. A unique verified exact alias can resolve locally; fuzzy or
/// ambiguous candidates require provider evaluation. No candidates or insufficient
/// evidence returns unresolved with null data, not an HTTP error. Transaction fields
/// and candidate records are sent to TypeSafe when evaluation runs. The overall
/// enrichment deadline is 55 seconds. Set `research: true` to research unresolved
/// merchants using OpenAI web search and persist a supported merchant/descriptor
/// alias with source evidence. This is opt-in, may incur research costs, and requires
/// server-side OPENAI_API_KEY. Aliases remain unverified. Failed or inconclusive
/// research creates no merchant or alias. Configuration, database and provider failures
/// return 502 rather than unresolved.
#[utoipa::path(post, path = "/v1/enrich", tag = "Enrichment",
    request_body(content = EnrichRequest, example = json!({"description":"LS","amount":"142.97","currency":"CAD","country":"CA","extra":{"bank_category":["Food and Drink","Restaurants"]}})),
    responses(
        (status = 200, description = "Completed merchant and location enrichment", body = EnrichResponse,
            examples(
                ("extracted_location" = (value = json!({"merchant":{"status":"unresolved","data":null},"location":{"status":"extracted","data":{"id":null,"precision":"city","address":null,"city":"Hialeah","region":"FL","postal_code":null,"country":"US","store_number":"10241"}}}))),
                ("unresolved" = (value = json!({"merchant":{"status":"unresolved","data":null},"location":{"status":"unresolved","data":null}}))),
                ("matched" = (value = json!({"merchant":{"status":"matched","data":{"id":"mer_example","name":"Example Café","markets":["CA"],"logo_url":"https://example.com/logo.png","logo_source":"official website"}},"location":{"status":"unresolved","data":null}})))
            )),
        (status = 400, description = "Malformed JSON", body = ApiError),
        (status = 413, description = "Request body exceeds 64 KiB", body = ApiError),
        (status = 415, description = "Missing or unsupported JSON content type", body = ApiError),
        (status = 422, description = "Invalid fields, unknown fields, or failed validation", body = ApiError,
            example = json!({"error":{"code":"invalid_request","message":"country must be a two-letter uppercase code"}})),
        (status = 503, description = "Database schema incompatible with this release", body = ApiError),
        (status = 502, description = "Enrichment failed, including configuration or provider errors", body = ApiError,
            example = json!({"error":{"code":"enrichment_failed","message":"Merchant evaluation failed"}})),
        (status = 504, description = "Enrichment exceeded the 55-second deadline", body = ApiError,
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
    match tokio::time::timeout(Duration::from_secs(55), enricher.enrich(&request)).await {
        Ok(Ok(result)) => Json(EnrichResponse::from(result)).into_response(),
        Ok(Err(error)) => {
            eprintln!("Enrichment failed: {error:#}");
            if let Some(response) = schema_error(&error) {
                return response;
            }
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

/// Enrich 1 to 100 transactions, preserving input order.
/// Each transaction has an isolated provider question. Verified exact matches and
/// empty candidate sets resolve locally. Provider calls are packed within byte
/// budgets, with at most four in flight. Invalid transactions and provider failures
/// produce per-item errors alongside successful results. The body limit is 1 MiB;
/// the overall deadline is 55 seconds, after which the whole request returns 504.
#[utoipa::path(post, path = "/v1/enrich/batch", tag = "Enrichment",
    request_body(content = BatchEnrichRequest, example = json!({"transactions":[{"description":"EXAMPLE CAFE","country":"CA"}]})),
    responses(
        (status = 200, description = "Ordered results, including per-item failures", body = BatchEnrichResponse),
        (status = 400, description = "Malformed JSON", body = ApiError),
        (status = 413, description = "Request body exceeds 1 MiB", body = ApiError),
        (status = 415, description = "Missing or unsupported JSON content type", body = ApiError),
        (status = 422, description = "Invalid envelope or transaction count", body = ApiError),
        (status = 504, description = "Batch exceeded the 55-second deadline", body = ApiError)
    )
)]
async fn enrich_batch(
    State(enricher): State<Arc<Enricher>>,
    payload: Result<Json<BatchEnrichRequest>, JsonRejection>,
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
    match tokio::time::timeout(
        Duration::from_secs(55),
        enricher.enrich_batch(&request.transactions),
    )
    .await
    {
        Ok(outcomes) => {
            let results = request
                .transactions
                .iter()
                .zip(outcomes)
                .map(|(request, outcome)| match outcome {
                    Ok(data) => BatchItemResult::Success {
                        data: Box::new(data.into()),
                    },
                    Err(error) => {
                        if let Err(validation) = request.validate() {
                            BatchItemResult::Error {
                                code: "invalid_request".into(),
                                message: validation.to_string(),
                            }
                        } else {
                            eprintln!("Batch enrichment failed: {error:#}");
                            BatchItemResult::Error {
                                code: "enrichment_failed".into(),
                                message: "Merchant evaluation failed".into(),
                            }
                        }
                    }
                })
                .collect();
            Json(BatchEnrichResponse { results }).into_response()
        }
        Err(_) => api_error(
            StatusCode::GATEWAY_TIMEOUT,
            "enrichment_timeout",
            "Merchant batch evaluation timed out",
        ),
    }
}

fn schema_error(error: &anyhow::Error) -> Option<Response> {
    error
        .downcast_ref::<ultrafinance_core::store::SchemaMismatch>()
        .map(|mismatch| {
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "database_schema_mismatch",
                &mismatch.to_string(),
            )
        })
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
    let database_url = env::var("ULTRAFINANCE_DATABASE_URL").ok();
    if env::var("ULTRAFINANCE_REQUIRE_POSTGRES").as_deref() == Ok("true") && database_url.is_none()
    {
        anyhow::bail!("ULTRAFINANCE_DATABASE_URL is required in production");
    }
    let store = ultrafinance_core::store::MerchantStore::postgres_lazy(
        database_url
            .as_deref()
            .unwrap_or(ultrafinance_core::store::LOCAL_DATABASE_URL),
    )?;
    let threshold = env::var("ULTRAFINANCE_MATCH_THRESHOLD")
        .unwrap_or_else(|_| "0.90".into())
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
#[path = "../tests/unit/main/tests.rs"]
mod tests;
