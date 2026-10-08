//! Public response records. Matching aliases stay in the internal catalog.
use serde::Serialize;
use ultrafinance_core::{LocationResult, markets::MarketEvidence};
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct Merchant {
    /// Stable ID in this service's merchant catalog.
    id: String,
    name: String,
    /// Countries with evidence of operation. Coverage is not exhaustive.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    markets: Vec<String>,
    /// Source and qualitative confidence for each known market.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    market_evidence: Vec<MarketEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    website: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logo_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logo_source: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    sources: Vec<String>,
}

impl From<ultrafinance_core::Merchant> for Merchant {
    fn from(value: ultrafinance_core::Merchant) -> Self {
        Self {
            id: value.id,
            name: value.name,
            markets: value.markets,
            market_evidence: value.market_evidence,
            website: value.website,
            logo_url: value.logo_url,
            logo_source: value.logo_source,
            sources: value.sources,
        }
    }
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MerchantResult {
    /// A supported catalog match, with its public merchant record.
    Matched { data: Merchant },
    /// Insufficient evidence or no candidates. The data field is always null.
    Unresolved {
        #[schema(schema_with = null_schema)]
        data: (),
    },
}

fn null_schema() -> utoipa::openapi::schema::Object {
    utoipa::openapi::schema::ObjectBuilder::new()
        .schema_type(utoipa::openapi::schema::Type::Null)
        .build()
}

#[derive(Serialize, ToSchema)]
pub struct EnrichResponse {
    merchant: MerchantResult,
    /// Independently extracted geography or a supported catalog outlet match.
    location: LocationResult,
    /// Source attribution for matched merchants and outlets. Omitted when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    attributions: Vec<String>,
}

impl From<ultrafinance_core::EnrichResponse> for EnrichResponse {
    fn from(value: ultrafinance_core::EnrichResponse) -> Self {
        Self {
            merchant: match value.merchant {
                ultrafinance_core::MerchantResult::Matched { data } => MerchantResult::Matched {
                    data: (*data).into(),
                },
                ultrafinance_core::MerchantResult::Unresolved { data } => {
                    MerchantResult::Unresolved { data }
                }
            },
            location: value.location,
            attributions: value.attributions,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct MerchantPage {
    merchants: Vec<Merchant>,
    total: usize,
    limit: usize,
    offset: usize,
}

impl From<ultrafinance_core::store::MerchantPage> for MerchantPage {
    fn from(value: ultrafinance_core::store::MerchantPage) -> Self {
        Self {
            merchants: value.merchants.into_iter().map(Into::into).collect(),
            total: value.total,
            limit: value.limit,
            offset: value.offset,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct BatchEnrichResponse {
    pub results: Vec<BatchItemResult>,
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BatchItemResult {
    Success { data: Box<EnrichResponse> },
    Error { code: String, message: String },
}
