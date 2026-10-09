pub mod batch;
mod columns;
pub mod datasets;
pub mod dedupe;
mod descriptor_location;
pub mod discovery;
pub mod eval;
pub mod foursquare;
pub mod gazetteer;
pub mod import;
mod import_progress;
pub mod interpretation;
pub mod location;
pub mod location_dedupe;
pub mod markets;
pub use location::{LocationData, LocationHint, LocationPrecision, LocationResult};
mod pipeline;
mod regex_rules;
pub mod resolution;
mod search_score;
pub mod store;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::json;
use serde_json::{Map, Value};
use std::{collections::HashSet, time::Duration};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct EnrichRequest {
    /// Bank transaction description. Must contain nonblank text and be at most 4096 UTF-8 bytes.
    pub description: String,
    /// Decimal amount as a string. The service does not validate its numeric format.
    pub amount: Option<String>,
    /// Three uppercase ASCII letters, for example CAD.
    #[cfg_attr(feature = "openapi", schema(pattern = "^[A-Z]{3}$"))]
    pub currency: Option<String>,
    /// Transaction date, conventionally YYYY-MM-DD. The service does not validate its format.
    pub date: Option<String>,
    /// Transaction country, as two uppercase ASCII letters. Known markets improve ranking; missing markets never exclude a merchant.
    #[cfg_attr(feature = "openapi", schema(pattern = "^[A-Z]{2}$"))]
    pub country: Option<String>,
    /// Known transaction geography. Used as evidence, never as a merchant country restriction.
    #[serde(default)]
    pub location: Option<LocationHint>,
    /// Additional evidence as an object with arbitrary JSON values. Sent to the provider when evaluation runs.
    #[serde(default)]
    pub extra: Map<String, Value>,
}

impl EnrichRequest {
    pub fn validate(&self) -> Result<()> {
        if let Some(location) = &self.location {
            location.validate()?;
        }
        if self.description.trim().is_empty() || self.description.len() > 4096 {
            bail!("description must contain 1 to 4096 bytes of nonblank text");
        }
        if self
            .country
            .as_ref()
            .is_some_and(|value| value.len() != 2 || !value.bytes().all(|c| c.is_ascii_uppercase()))
        {
            bail!("country must be a two-letter uppercase code");
        }
        if self
            .currency
            .as_ref()
            .is_some_and(|value| value.len() != 3 || !value.bytes().all(|c| c.is_ascii_uppercase()))
        {
            bail!("currency must be a three-letter uppercase code");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Merchant {
    /// Stable ID in this service's merchant catalog.
    pub id: String,
    pub name: String,
    /// Countries with evidence of operation. Coverage is not exhaustive.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markets: Vec<String>,
    /// Source and qualitative confidence for each known market.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub market_evidence: Vec<markets::MarketEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_source: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
}

// Internally tagged variants ensure status and data cannot disagree.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MerchantResult {
    /// A supported catalog match, with its merchant record.
    Matched { data: Box<Merchant> },
    /// Insufficient evidence or no candidates. The data field is always null.
    Unresolved {
        #[cfg_attr(feature = "openapi", schema(schema_with = null_schema))]
        data: (),
    },
}

#[cfg(feature = "openapi")]
fn null_schema() -> utoipa::openapi::schema::Object {
    utoipa::openapi::schema::ObjectBuilder::new()
        .schema_type(utoipa::openapi::schema::Type::Null)
        .build()
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EnrichResponse {
    pub merchant: MerchantResult,
    /// Independently extracted geography or a supported catalog outlet match.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(required = true))]
    pub location: LocationResult,
    /// Source attribution for matched merchants and outlets. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributions: Vec<String>,
}

#[derive(Clone)]
pub struct Enricher {
    persist_matches: bool,
    discovery: Option<discovery::Discovery>,
    client: reqwest::Client,
    provider_url: String,
    api_key: Option<String>,
    model: String,
    threshold: f64,
    store: store::MerchantStore,
}

impl Enricher {
    pub fn new(
        api_key: Option<String>,
        model: String,
        threshold: f64,
        merchants: Vec<Merchant>,
    ) -> Result<Self> {
        if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
            bail!("match threshold must be between 0 and 1");
        }
        let store = store::MerchantStore::temporary()?;
        let mut ids = HashSet::new();
        for merchant in &merchants {
            if !ids.insert(&merchant.id) {
                bail!("merchant IDs must be unique");
            }
            store.put(merchant)?;
        }
        Self::with_store(api_key, model, threshold, store)
    }

    pub fn with_store(
        api_key: Option<String>,
        model: String,
        threshold: f64,
        store: store::MerchantStore,
    ) -> Result<Self> {
        if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
            bail!("match threshold must be between 0 and 1");
        }
        Ok(Self {
            persist_matches: true,
            discovery: discovery::Discovery::from_env()?,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()?,
            provider_url: "https://api.typesafe.ai/v1/systemone".into(),
            api_key: api_key.filter(|key| !key.trim().is_empty()),
            model,
            threshold,
            store,
        })
    }

    /// Evaluate decisions without learning resolutions or importing discovered merchants.
    pub(crate) fn for_evaluation(
        api_key: Option<String>,
        model: String,
        threshold: f64,
        store: store::MerchantStore,
    ) -> Result<Self> {
        let mut enricher = Self::with_store(api_key, model, threshold, store)?;
        enricher.persist_matches = false;
        Ok(enricher)
    }

    /// Browse the catalog, or paginate a bounded pool of ranked search candidates.
    /// This read-only operation never evaluates candidates with the AI provider.
    pub async fn list_merchants(
        &self,
        query: Option<String>,
        market: Option<String>,
        limit: usize,
        offset: usize,
    ) -> Result<store::MerchantPage> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(query) = query {
                let mut candidates = store.search(&query, market.as_deref(), 100)?;
                if let Some(market) = &market {
                    candidates.retain(|c| c.merchant.markets.contains(market));
                }
                let total = candidates.len();
                Ok(store::MerchantPage {
                    merchants: candidates
                        .into_iter()
                        .skip(offset)
                        .take(limit)
                        .map(|candidate| candidate.merchant)
                        .collect(),
                    total,
                    limit,
                    offset,
                })
            } else {
                store.list(market.as_deref(), limit, offset)
            }
        })
        .await?
    }

    pub async fn enrich(&self, request: &EnrichRequest) -> Result<EnrichResponse> {
        self.enrich_batch(std::slice::from_ref(request))
            .await
            .remove(0)
    }

    /// Return evidence from the same enrichment run, including failed decisions.
    pub async fn enrich_with_details(
        &self,
        request: &EnrichRequest,
    ) -> (Result<EnrichResponse>, Value) {
        let candidates = match request.validate() {
            Err(error) => Err(error),
            Ok(()) => self.retrieve(request).await,
        };
        let (mut results, mut details) = self
            .enrich_batch_candidates_traced(vec![(request.clone(), candidates)])
            .await;
        (
            results.remove(0),
            details.pop().unwrap_or_else(|| serde_json::json!({})),
        )
    }
}

fn unresolved() -> EnrichResponse {
    EnrichResponse {
        location: LocationResult::default(),
        attributions: vec![],
        merchant: MerchantResult::Unresolved { data: () },
    }
}

#[cfg(test)]
fn parse_choice(value: &Value, candidates: &[Merchant], threshold: f64) -> Result<EnrichResponse> {
    parse_choice_answer(&value["answers"]["merchant"], candidates, threshold)
}

fn parse_choice_answer(
    answer: &Value,
    candidates: &[Merchant],
    threshold: f64,
) -> Result<EnrichResponse> {
    if answer["type"] != "choice" {
        bail!("Jev returned an unexpected answer type");
    }
    let choice = answer["choice"]
        .as_str()
        .context("Jev answer is missing choice")?;
    let confidence = answer["confidence"]
        .as_f64()
        .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
        .context("Jev returned invalid confidence")?;
    let probabilities = answer["probabilities"]
        .as_object()
        .context("Jev answer is missing probabilities")?;
    let probability = probabilities
        .get(choice)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
        .context("Jev returned invalid choice probability")?;
    if choice == "none" {
        return Ok(unresolved());
    }
    let candidate = candidates
        .iter()
        .enumerate()
        .find(|(index, _)| choice == format!("candidate_{index}"))
        .map(|(_, merchant)| merchant)
        .context("Jev chose an unknown candidate")?;
    if confidence < threshold || probability < threshold {
        return Ok(unresolved());
    }
    Ok(EnrichResponse {
        location: LocationResult::default(),
        attributions: vec![],
        merchant: MerchantResult::Matched {
            data: Box::new(candidate.clone()),
        },
    })
}

/// Load a configured catalog. Only the missing default catalog is treated as empty.
pub fn load_catalog(path: Option<&std::path::Path>) -> Result<Vec<Merchant>> {
    let default = std::path::Path::new("data/merchants.json");
    let selected = path.unwrap_or(default);
    match std::fs::read_to_string(selected) {
        Ok(contents) => serde_json::from_str(&contents).context("invalid merchant catalog"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && path.is_none() => {
            eprintln!(
                "No merchant catalog at {}; enrichment will return unresolved",
                selected.display()
            );
            Ok(Vec::new())
        }
        Err(error) => Err(error)
            .with_context(|| format!("cannot read merchant catalog {}", selected.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn candidates() -> Vec<Merchant> {
        serde_json::from_value(json!([{"id":"mer_1","name":"Example Café","markets":["CA"]}]))
            .unwrap()
    }
    #[tokio::test]
    async fn exact_matches_bypass_jev_but_collisions_and_fuzzy_candidates_do_not() {
        let store = store::MerchantStore::temporary().unwrap();
        let merchant = candidates().remove(0);
        store.put(&merchant).unwrap();
        let request: EnrichRequest =
            serde_json::from_value(json!({"description":"EXAMPLE CAFE","country":"CA"})).unwrap();
        let enricher =
            Enricher::with_store(None, "jev-latest".into(), 0.95, store.clone()).unwrap();
        assert!(matches!(
            enricher.enrich(&request).await.unwrap().merchant,
            MerchantResult::Matched { .. }
        ));
        let mut fuzzy = request.clone();
        fuzzy.description = "Exampl cafe".into();
        assert!(
            enricher
                .enrich(&fuzzy)
                .await
                .unwrap_err()
                .to_string()
                .contains("not configured")
        );
        let mut noisy = request.clone();
        noisy.description =
            "CREDIT CARD PURCHASE EXAMPLE CAFE STORE 1005 MONTREAL QC 2026 10 01".into();
        let retrieved = store.search(&noisy.description, Some("CA"), 10).unwrap();
        assert_eq!(retrieved[0].merchant.id, merchant.id);
        assert!(!retrieved[0].exact);
        assert!(
            enricher
                .enrich(&noisy)
                .await
                .unwrap_err()
                .to_string()
                .contains("not configured")
        );
        let mut other = merchant.clone();
        other.id = "mer_2".into();
        store.put(&other).unwrap();
        assert!(
            enricher
                .enrich(&request)
                .await
                .unwrap_err()
                .to_string()
                .contains("not configured")
        );
    }

    #[test]
    fn catalogs_larger_than_jev_choice_limit_are_supported() {
        let merchants: Vec<_> = (0..300)
            .map(|index| Merchant {
                id: format!("mer_{index}"),
                name: format!("Merchant {index}"),
                markets: vec!["CA".into()],
                market_evidence: vec![],
                website: None,
                logo_url: None,
                logo_source: None,
                aliases: vec![],
                sources: vec![],
            })
            .collect();
        let enricher = Enricher::new(None, "jev-latest".into(), 0.95, merchants).unwrap();
        let candidates = enricher
            .store
            .search("Merchant 299", Some("CA"), 10)
            .unwrap();
        assert!(candidates.len() <= 10);
        assert_eq!(candidates[0].merchant.id, "mer_299");
        assert!(candidates[0].exact);
    }

    #[test]
    fn extra_preserves_nested_evidence_and_typos_fail() {
        let value = json!({"description":"LS","extra":{"plaid":{"category":["Restaurants"]}}});
        let request: EnrichRequest = serde_json::from_value(value).unwrap();
        assert_eq!(request.extra["plaid"]["category"][0], "Restaurants");
        assert!(
            serde_json::from_value::<EnrichRequest>(json!({"description":"LS","contry":"CA"}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<EnrichRequest>(json!({"description":" "}))
                .unwrap()
                .validate()
                .is_err()
        );
    }
    #[test]
    fn match_requires_valid_candidate_and_sufficient_evidence() {
        let candidates = candidates();
        let mut response = json!({"answers":{"merchant":{"type":"choice","choice":"candidate_0","confidence":0.96,"probabilities":{"candidate_0":0.98,"none":0.02}}}});
        assert!(matches!(
            parse_choice(&response, &candidates, 0.9).unwrap().merchant,
            MerchantResult::Matched { .. }
        ));
        response["answers"]["merchant"]["confidence"] = json!(0.5);
        assert!(matches!(
            parse_choice(&response, &candidates, 0.9).unwrap().merchant,
            MerchantResult::Unresolved { .. }
        ));
        response["answers"]["merchant"]["choice"] = json!("candidate_99");
        response["answers"]["merchant"]["probabilities"]["candidate_99"] = json!(0.9);
        assert!(parse_choice(&response, &candidates, 0.9).is_err());
        assert_eq!(
            serde_json::to_value(unresolved()).unwrap(),
            json!({"merchant":{"status":"unresolved","data":null},"location":{"status":"unresolved","data":null}})
        );
    }
}
