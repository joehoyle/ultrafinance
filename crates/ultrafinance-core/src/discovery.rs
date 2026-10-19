//! Optional structured business-discovery adapter. Listings are evidence, not trust.
use crate::{
    EnrichRequest, Merchant,
    interpretation::interpret,
    store::{Candidate, SourceRecord},
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

#[derive(Clone)]
pub struct Discovery {
    pub(crate) url: String,
    api_key: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Place {
    pub source: String,
    pub external_id: String,
    pub name: String,
    pub country: Option<String>,
    pub website: Option<String>,
    pub evidence_url: String,
    pub evidence: String,
    pub attribution: String,
    pub license: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    places: Vec<Place>,
}
impl Discovery {
    pub fn from_env() -> Result<Option<Self>> {
        let Some(url) = std::env::var("ULTRAFINANCE_DISCOVERY_URL")
            .ok()
            .filter(|v| !v.is_empty())
        else {
            return Ok(None);
        };
        let parsed = reqwest::Url::parse(&url).context("invalid discovery URL")?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            bail!("discovery URL must use HTTPS without embedded credentials");
        }
        Ok(Some(Self {
            url,
            api_key: std::env::var("ULTRAFINANCE_DISCOVERY_API_KEY")
                .ok()
                .filter(|k| !k.is_empty()),
        }))
    }
    pub(crate) async fn candidates(&self, request: &EnrichRequest) -> Result<Vec<Candidate>> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        // Send only the descriptor, geography and hypotheses, never account extras or amounts.
        let mut call = client.post(&self.url).json(&json!({"description":request.description,
            "country":request.country,"location":request.location,"interpretation":interpret(request),"limit":5}));
        if let Some(key) = &self.api_key {
            call = call.bearer_auth(key);
        }
        let response = call
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("business discovery request failed"))?;
        if !response.status().is_success() {
            bail!("business discovery returned HTTP {}", response.status());
        }
        if response.content_length().is_some_and(|n| n > 64 * 1024) {
            bail!("business discovery response exceeds 64 KiB");
        }
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("business discovery response failed"))?
        {
            if bytes.len() + chunk.len() > 64 * 1024 {
                bail!("business discovery response exceeds 64 KiB");
            }
            bytes.extend_from_slice(&chunk);
        }
        let response: Response =
            serde_json::from_slice(&bytes).context("invalid business discovery response")?;
        if response.places.len() > 5 {
            bail!("business discovery returned more than five places");
        }
        response.places.into_iter().map(candidate).collect()
    }
}
fn candidate(place: Place) -> Result<Candidate> {
    for text in [
        &place.source,
        &place.external_id,
        &place.name,
        &place.evidence,
        &place.attribution,
        &place.license,
    ] {
        if text.trim().is_empty() || text.len() > 4096 {
            bail!(
                "discovered places require nonblank bounded identity, evidence, attribution and license"
            );
        }
    }
    for url in std::iter::once(&place.evidence_url).chain(place.website.iter()) {
        let url = reqwest::Url::parse(url).context("invalid discovered place URL")?;
        if !["http", "https"].contains(&url.scheme())
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            bail!("invalid discovered place URL");
        }
    }
    if place
        .country
        .as_ref()
        .is_some_and(|c| c.len() != 2 || !c.bytes().all(|b| b.is_ascii_uppercase()))
    {
        bail!("invalid discovered place country");
    }
    let merchant = Merchant {
        id: format!("discovery_{}", uuid::Uuid::new_v4().simple()),
        name: place.name.clone(),
        markets: place.country.iter().cloned().collect(),

        website: place.website.clone(),
        logo_url: None,
        logo_source: None,
        aliases: vec![],
        sources: vec![],
    };
    let record = SourceRecord {
        source: format!("discovery:{}", place.source),
        external_id: place.external_id.clone(),
        merchant: merchant.clone(),
        attribution: place.attribution.clone(),
        license: place.license.clone(),
        url: place.evidence_url.clone(),
        version: None,
        raw: json!({"listing":place,"verification":"discovery-listing"}),
    };
    Ok(Candidate {
        merchant,
        score: 0.0,
        exact: false,
        regex_match_length: None,
        trusted: false,
        provenance: vec![record],
        resolution_id: None,
        pending_import: true,
        interpretation_evidence: vec![],
        retrieval_evidence: vec![],
    })
}

#[cfg(test)]
#[path = "../tests/unit/discovery/tests.rs"]
mod tests;
