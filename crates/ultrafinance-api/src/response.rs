//! Public response records. Matching aliases and source URLs stay in the internal catalog.
use serde::Serialize;
use ultrafinance_core::LocationResult;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct Merchant {
    /// Foursquare business categories, with paths ordered from root to leaf. Omitted when unknown.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    categories: Vec<ultrafinance_core::categories::Category>,
    /// Stable ID in this service's merchant catalog.
    id: String,
    name: String,
    /// Known countries of operation. Coverage is not exhaustive.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    markets: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    website: Option<String>,
    /// Brand image URL, when available. Resolve /logos/assets/... paths against the API origin.
    #[serde(skip_serializing_if = "Option::is_none")]
    logo_url: Option<String>,
}

impl From<ultrafinance_core::Merchant> for Merchant {
    fn from(value: ultrafinance_core::Merchant) -> Self {
        Self {
            categories: value
                .categories
                .unwrap_or_else(|| ultrafinance_core::categories::resolve(&value.category_ids)),
            id: value.id,
            name: value.name,
            markets: value.markets,
            website: value.website,
            logo_url: value.logo_url,
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
pub struct Counterparty {
    #[serde(rename = "type")]
    counterparty_type: ultrafinance_core::CounterpartyType,
    #[serde(flatten)]
    identity: Merchant,
}

impl From<ultrafinance_core::Counterparty> for Counterparty {
    fn from(value: ultrafinance_core::Counterparty) -> Self {
        Self {
            counterparty_type: value.counterparty_type,
            identity: value.into(),
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct EnrichResponse {
    /// Financial operation classified independently of merchant identity; unknown when evidence is insufficient or Jev is not configured.
    kind: ultrafinance_core::TransactionKind,
    merchant: MerchantResult,
    /// Independently supported merchants, institutions and payment processors; empty when unresolved.
    counterparties: Vec<Counterparty>,
    /// Independently extracted geography or a supported catalog outlet match.
    location: LocationResult,
}

impl From<ultrafinance_core::EnrichResponse> for EnrichResponse {
    fn from(value: ultrafinance_core::EnrichResponse) -> Self {
        Self {
            kind: value.kind,
            counterparties: value.counterparties.into_iter().map(Into::into).collect(),
            merchant: match value.merchant {
                ultrafinance_core::MerchantResult::Matched { data } => MerchantResult::Matched {
                    data: (*data).into(),
                },
                ultrafinance_core::MerchantResult::Unresolved { data } => {
                    MerchantResult::Unresolved { data }
                }
            },
            location: value.location,
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

#[derive(Serialize, ToSchema)]
pub struct CounterpartyPage {
    counterparties: Vec<Counterparty>,
    total: usize,
    limit: usize,
    offset: usize,
}
impl From<ultrafinance_core::store::MerchantPage> for CounterpartyPage {
    fn from(value: ultrafinance_core::store::MerchantPage) -> Self {
        Self {
            counterparties: value.merchants.into_iter().map(Into::into).collect(),
            total: value.total,
            limit: value.limit,
            offset: value.offset,
        }
    }
}
