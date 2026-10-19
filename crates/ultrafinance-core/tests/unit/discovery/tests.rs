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
            let mut response: serde_json::Value = serde_json::from_str(&response).unwrap();
            if let Some(questions) = request["questions"].as_object() {
                for key in questions.keys().filter(|key| key.starts_with("kind_")) {
                    response["answers"][key] = json!({"type":"choice","choice":"refund","confidence":0.99,"probabilities":{"refund":0.99}});
                }
            }
            let response = response.to_string();
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
    let (url, provider_rx) = server(provider("candidate_0"), 200, 3);
    enricher.provider_url = url;
    let request: EnrichRequest = serde_json::from_value(
        json!({"description":"opaque zxmq","country":"CA","amount":"10.00","extra":{"private":"do not forward to discovery"}}),
    )?;
    let first = enricher.enrich(&request).await?;
    assert_eq!(first.kind, crate::TransactionKind::Refund);
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
    let mut merchant_questions = 0;
    for _ in 0..3 {
        let body = provider_rx.recv().unwrap();
        if body["questions"].get("transaction_0").is_some() {
            assert!(
                body["questions"]["transaction_0"]["instructions"]["interpretation"].is_object()
            );
            merchant_questions += 1;
        }
    }
    assert_eq!(merchant_questions, 2);
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
async fn evaluation_discovery_matches_do_not_import_or_remember_evidence() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let mut enricher = Enricher::for_evaluation(
        Some("test-key".into()),
        "jev-latest".into(),
        0.95,
        store.clone(),
    )?;
    let (url, discovery_rx) = server(listing(), 200, 1);
    enricher.discovery = Some(Discovery { url, api_key: None });
    let (url, provider_rx) = server(provider("candidate_0"), 200, 2);
    enricher.provider_url = url;
    let request = serde_json::from_value(json!({"description":"opaque zxmq","country":"CA"}))?;
    assert!(matches!(
        enricher.enrich(&request).await?.merchant,
        MerchantResult::Matched { .. }
    ));
    discovery_rx.recv().unwrap();
    provider_rx.recv().unwrap();
    provider_rx.recv().unwrap();
    assert_eq!(store.stats()?.total, 0);
    assert!(store.resolutions(None, 10)?.is_empty());
    assert!(store.enrichment_logs(None, None, 10, 0)?.is_empty());
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
        let (url, provider_rx) = server(provider(choice), 200, if error { 1 } else { 2 });
        enricher.provider_url = url;
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
        for _ in 0..if error { 1 } else { 2 } {
            provider_rx.recv().unwrap();
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
    let (url, provider_rx) = server(json!({"auto_choice":"none"}), 200, 2);
    enricher.provider_url = url;
    let requests: Vec<EnrichRequest> = (0..6)
        .map(|i| serde_json::from_value(json!({"description":format!("opaque zxmq {i}")})).unwrap())
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
