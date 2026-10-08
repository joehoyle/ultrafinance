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
        market_evidence: vec![],
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Enricher, MerchantResult, store::MerchantStore};
    use std::{
        io::{Read, Write},
        sync::mpsc,
    };
    fn server(
        body: serde_json::Value,
        status: u16,
        count: usize,
    ) -> (String, mpsc::Receiver<serde_json::Value>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for index in 0..count {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = vec![];
                let (end, len) = loop {
                    let mut b = [0; 4096];
                    let n = stream.read(&mut b).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&b[..n]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                        let len = headers
                            .lines()
                            .find_map(|s| {
                                s.strip_prefix("content-length:")
                                    .map(|s| s.trim().parse::<usize>().unwrap())
                            })
                            .unwrap();
                        break (end + 4, len);
                    }
                };
                while bytes.len() < end + len {
                    let mut b = [0; 4096];
                    let n = stream.read(&mut b).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&b[..n]);
                }
                let request: serde_json::Value =
                    serde_json::from_slice(&bytes[end..end + len]).unwrap();
                let response = if let Some(choice) = body["auto_choice"].as_str() {
                    let answers: serde_json::Map<String,serde_json::Value> = request["questions"].as_object().unwrap().keys()
                        .map(|key|(key.clone(),json!({"type":"choice","choice":choice,"confidence":0.99,"probabilities":{choice:0.99}}))).collect();
                    json!({"answers":answers}).to_string()
                } else if body.is_array() {
                    body[index].to_string()
                } else {
                    body.to_string()
                };
                write!(stream,"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).unwrap();
                tx.send(request).unwrap();
            }
        });
        (url, rx)
    }
    fn listing() -> serde_json::Value {
        json!({"places":[{"source":"test-directory","external_id":"cafe-1","name":"Royal Cafe","country":"CA",
            "website":"https://example.com","evidence_url":"https://example.com/cafe",
            "evidence":"Listing associates opaque zxmq with Royal Cafe in Bromont, Quebec, Canada", "attribution":"Test Directory","license":"CC0-1.0"}]})
    }
    fn provider(choice: &str) -> serde_json::Value {
        json!({"answers":{"transaction_0":{"type":"choice","choice":choice,"confidence":0.99,"probabilities":{choice:0.99}}}})
    }
    #[tokio::test]
    async fn discovery_is_grounded_persisted_untrusted_and_reusable_after_review() -> Result<()> {
        let store = MerchantStore::temporary()?;
        let mut enricher = Enricher::with_store(
            Some("test-key".into()),
            "jev-latest".into(),
            0.95,
            store.clone(),
        )?;
        let (url, discovery_rx) = server(listing(), 200, 1);
        enricher.discovery = Some(Discovery { url, api_key: None });
        let (url, provider_rx) = server(provider("candidate_0"), 200, 2);
        enricher.provider_url = url;
        let request: EnrichRequest = serde_json::from_value(
            json!({"description":"opaque zxmq","country":"CA","amount":"10.00","extra":{"private":"do not forward to discovery"}}),
        )?;
        let first = enricher.enrich(&request).await?;
        let MerchantResult::Matched { data: first } = first.merchant else {
            panic!("expected supported discovered merchant")
        };
        assert!(!first.id.starts_with("discovery_"));
        assert_eq!(store.stats()?.total, 1);
        assert!(!first.aliases.iter().any(|s| s.contains("opaque")));
        let query = discovery_rx.recv().unwrap();
        assert!(query.get("amount").is_none() && query.get("extra").is_none());
        let second = enricher.enrich(&request).await?;
        let MerchantResult::Matched { data: second } = second.merchant else {
            panic!("expected cached candidate")
        };
        assert_eq!(first.id, second.id);
        for _ in 0..2 {
            let body = provider_rx.recv().unwrap();
            assert!(
                body["questions"]["transaction_0"]["instructions"]["interpretation"].is_object()
            );
        }
        let mut mapping = store.resolutions(None, 10)?.remove(0);
        assert!(!mapping.verified);
        mapping.verified = true;
        mapping.evidence = Some("Receipt checked against official cafe website".into());
        store.save_resolution(&mapping)?;
        let direct = Enricher::with_store(None, "jev-latest".into(), 0.95, store.clone())?;
        assert!(matches!(
            direct.enrich(&request).await?.merchant,
            MerchantResult::Matched { .. }
        ));
        let mut changed = request.clone();
        changed.country = Some("US".into());
        assert!(matches!(
            direct.enrich(&changed).await?.merchant,
            MerchantResult::Unresolved { .. }
        ));
        let logs = store.enrichment_logs(Some("matched"), None, 10, 0)?;
        assert!(
            logs.iter()
                .any(|r| r["data"]["method"] == "verified_descriptor")
        );
        Ok(())
    }
    #[tokio::test]
    async fn rejected_and_invalid_discoveries_do_not_pollute_the_catalog() -> Result<()> {
        for (response, status, choice, error) in [
            (listing(), 200, "none", false),
            (listing(), 503, "none", true),
            (
                json!({"places":[{"name":"no identity or evidence"}]}),
                200,
                "none",
                true,
            ),
        ] {
            let store = MerchantStore::temporary()?;
            let mut enricher = Enricher::with_store(
                Some("test-key".into()),
                "jev-latest".into(),
                0.95,
                store.clone(),
            )?;
            let (url, rx) = server(response, status, 1);
            enricher.discovery = Some(Discovery { url, api_key: None });
            let provider_rx = if !error {
                let (url, rx) = server(provider(choice), 200, 1);
                enricher.provider_url = url;
                Some(rx)
            } else {
                None
            };
            let request = serde_json::from_value(json!({"description":"opaque zxmq"}))?;
            let result = enricher.enrich(&request).await;
            assert_eq!(result.is_err(), error);
            if !error {
                assert!(matches!(
                    result?.merchant,
                    MerchantResult::Unresolved { .. }
                ));
            }
            assert_eq!(store.stats()?.total, 0);
            assert!(store.resolutions(None, 10)?.is_empty());
            rx.recv().unwrap();
            if let Some(rx) = provider_rx {
                rx.recv().unwrap();
            }
        }
        Ok(())
    }
    #[tokio::test]
    async fn discovery_runs_after_catalog_abstention_and_has_a_batch_budget() -> Result<()> {
        let store = MerchantStore::temporary()?;
        let merchant = serde_json::from_value(json!({"id":"royal","name":"Royal Cafe"}))?;
        store.put(&merchant)?;
        let mut enricher = Enricher::with_store(
            Some("test-key".into()),
            "jev-latest".into(),
            0.95,
            store.clone(),
        )?;
        let (url, rx) = server(listing(), 200, 1);
        enricher.discovery = Some(Discovery { url, api_key: None });
        let (url, provider_rx) = server(json!([provider("none"), provider("candidate_0")]), 200, 2);
        enricher.provider_url = url;
        let request =
            serde_json::from_value(json!({"description":"Royal Cafe payment 987","country":"CA"}))?;
        assert!(matches!(
            enricher.enrich(&request).await?.merchant,
            MerchantResult::Matched { .. }
        ));
        rx.recv().unwrap();
        provider_rx.recv().unwrap();
        provider_rx.recv().unwrap();
        let store = MerchantStore::temporary()?;
        let mut enricher = Enricher::with_store(
            Some("test-key".into()),
            "jev-latest".into(),
            0.95,
            store.clone(),
        )?;
        let (url, rx) = server(listing(), 200, 4);
        enricher.discovery = Some(Discovery { url, api_key: None });
        let (url, provider_rx) = server(json!({"auto_choice":"none"}), 200, 1);
        enricher.provider_url = url;
        let requests: Vec<EnrichRequest> = (0..6)
            .map(|i| {
                serde_json::from_value(json!({"description":format!("opaque zxmq {i}")})).unwrap()
            })
            .collect();
        let outcomes = enricher.enrich_batch(&requests).await;
        assert!(
            outcomes
                .into_iter()
                .all(|r| matches!(r.unwrap().merchant, MerchantResult::Unresolved { .. }))
        );
        for _ in 0..4 {
            rx.recv().unwrap();
        }
        provider_rx.recv().unwrap();
        let logs = store.enrichment_logs(None, None, 10, 0)?;
        assert_eq!(
            logs.iter()
                .filter(|r| r["data"]["method"] == "discovery")
                .count(),
            4
        );
        assert_eq!(store.stats()?.total, 0);
        Ok(())
    }
}
