use super::*;
use std::io::{Read, Write};
use std::sync::mpsc;

include!("processor_tests.rs");

fn request(description: &str) -> EnrichRequest {
    serde_json::from_value(json!({"description":description,"country":"CA"})).unwrap()
}
fn enricher() -> Enricher {
    let merchants = serde_json::from_value(json!([
        {"id":"alpha","name":"Alpha Cafe","markets":["CA"]},
        {"id":"beta","name":"Beta Shop","markets":["CA"]}
    ]))
    .unwrap();
    Enricher::new(
        Some("test-key".into()),
        "jev-latest".into(),
        0.95,
        merchants,
    )
    .unwrap()
}
fn candidates(enricher: &Enricher, name: &str) -> Vec<store::Candidate> {
    let mut candidates = enricher.store.search(name, Some("CA"), 10).unwrap();
    assert!(!candidates.is_empty());
    for candidate in &mut candidates {
        candidate.exact = false;
    }
    candidates
}
fn brand_fixture(
    id: &str,
    name: &str,
    website: &str,
    source: &str,
    external_id: &str,
) -> store::Candidate {
    let merchant: Merchant =
        serde_json::from_value(json!({"id":id,"name":name,"website":website,"markets":["CA"]}))
            .unwrap();
    store::Candidate {
        merchant: merchant.clone(),
        score: 0.9,
        exact: false,
        trusted: false,
        regex_match_length: None,
        resolution_id: None,
        pending_import: false,
        interpretation_evidence: vec![],
        retrieval_evidence: vec![],
        business_evidence: serde_json::Value::Null,
        provenance: vec![store::SourceRecord {
            merchant,
            source: source.into(),
            external_id: external_id.into(),
            raw: json!({}),
            attribution: "fixture".into(),
            license: "fixture".into(),
            url: "https://example.com".into(),
            version: None,
        }],
    }
}
#[tokio::test]
async fn same_name_businesses_are_resolved_after_name_recognition() {
    let mut enricher = enricher();
    let shortlisted = vec![
        brand_fixture(
            "place",
            "Alpha Cafe",
            "https://venue.example",
            "foursquare",
            "place:one",
        ),
        brand_fixture(
            "alpha",
            "Alpha Cafe",
            "https://alpha.example",
            "open-enrichment",
            "alpha",
        ),
        brand_fixture(
            "beta",
            "Beta Shop",
            "https://beta.example",
            "merchant-studio",
            "beta",
        ),
    ];
    let rx = mock_responses(&mut enricher, 2, 200, |body| {
        let answers: Map<_, _> = body["questions"]
            .as_object()
            .unwrap()
            .keys()
            .map(|key| {
                (
                    key.clone(),
                    answer(if key.starts_with("kind_") {
                        "purchase"
                    } else if key.starts_with("identity_") {
                        "candidate_1"
                    } else {
                        "candidate_0"
                    }),
                )
            })
            .collect();
        json!({"answers":answers})
    });
    let (mut results, details) = enricher
        .enrich_batch_candidates_traced(vec![(request("Alpha Cafe ON CAN"), Ok(shortlisted))])
        .await;
    assert!(
        matches!(results.remove(0).unwrap().merchant, MerchantResult::Matched { data } if data.id == "alpha")
    );
    let sent = rx.recv().unwrap();
    let names = &sent["questions"]["transaction_0"]["criteria"];
    assert_eq!(names.as_object().unwrap().len(), 3);
    assert_eq!(names["candidate_0"]["name"], "Alpha Cafe");
    assert!(names.get("candidate_1").is_none());
    assert!(!names.to_string().contains("https://"));
    let identities = rx.recv().unwrap();
    let choices = &identities["questions"]["identity_0"]["criteria"];
    assert_eq!(choices.as_object().unwrap().len(), 3);
    assert_eq!(
        choices["candidate_0"]["merchant"]["website"],
        "https://venue.example"
    );
    assert_eq!(
        choices["candidate_1"]["merchant"]["website"],
        "https://alpha.example"
    );
    assert!(choices.get("candidate_2").is_none());
    assert_eq!(
        details[0]["provider_answer"]["name_answer"]["choice"],
        "candidate_0"
    );
    assert_eq!(details[0]["provider_answer"]["decision_stage"], "identity");
    assert_eq!(details[0]["provider_requests"].as_array().unwrap().len(), 2);
    assert_eq!(
        details[0]["provider_choices"][1]["records"][0]["id"],
        "alpha"
    );
    assert_eq!(
        details[0]["provider_answer"]["catalog_resolution"],
        "resolved"
    );
}
#[test]
fn provider_projection_retains_distinct_evidence_and_caller_context() {
    let mut candidate = brand_fixture(
        "internal-one",
        "Alpha Cafe",
        "https://alpha.example",
        "foursquare",
        "place:internal-one",
    );
    candidate.merchant.aliases = vec!["ALPHA CAFE".into(), "Alpha".into(), "ALPHA".into()];
    candidate.provenance[0].merchant = candidate.merchant.clone();
    candidate.provenance[0].raw = json!({"parent_id":"internal-parent",
        "negativeAliases":["OTHER CAFE"],"transaction_text_regexp":"^ALPHA",
        "countryHints":["CA"],"unused":"irrelevant-source-data"});
    let evidence = candidate_evidence(&candidate);
    assert_eq!(
        evidence["merchant"],
        json!({"name":"Alpha Cafe", "type":"merchant",
        "aliases":["Alpha"],"markets":["CA"],"website":"https://alpha.example","business_domain":"alpha.example"})
    );
    assert_eq!(
        evidence["provenance"],
        json!([{"source":"foursquare","raw":{
        "negativeAliases":["OTHER CAFE"],"transaction_text_regexp":"^ALPHA","countryHints":["CA"]}}])
    );
    assert!(evidence.get("interpretation_evidence").is_none());
    let before = serde_json::to_value(&candidate).unwrap();
    let duplicate = candidate.clone();
    let mut alternative = candidate.clone();
    alternative.merchant.id = "internal-two".into();
    alternative.merchant.website = Some("https://other.example".into());
    alternative.provenance[0].merchant = alternative.merchant.clone();
    let request: EnrichRequest = serde_json::from_value(json!({
        "description":"SQ* ALPHA BROMONT", "amount":"12.34", "currency":"CAD",
        "date":"2026-10-09", "country":"CA", "location":{"city":"Bromont"},
        "extra":{"id":"caller-id","nested":{"unknown":null,"empty":[]},"notes":"lunch"}
    }))
    .unwrap();
    let sent = question(&request, &[candidate.clone(), duplicate, alternative]);
    let primary = &sent["criteria"]["candidate_0"];
    assert_eq!(primary["merchant"]["website"], "https://alpha.example");
    assert_eq!(
        sent["criteria"]["candidate_1"]["merchant"]["website"],
        "https://other.example"
    );
    assert_eq!(sent["criteria"].as_object().unwrap().len(), 3);
    assert!(!sent.to_string().contains("internal-"));
    assert!(!sent.to_string().contains("irrelevant-source-data"));
    let transaction = &sent["instructions"]["transaction"];
    assert_eq!(transaction["extra"], json!(request.extra));
    assert_eq!(transaction["location"], json!({"city":"Bromont"}));
    for field in ["description", "amount", "currency", "date", "country"] {
        assert_eq!(transaction[field], json!(request)[field]);
    }
    assert!(
        sent["instructions"]["interpretation"]
            .get("original")
            .is_none()
    );
    assert!(
        !sent["instructions"]["interpretation"]
            .to_string()
            .contains("geoname_ids")
    );
    let minimal: EnrichRequest = serde_json::from_value(json!({"description":"ALPHA"})).unwrap();
    assert_eq!(
        transaction_evidence(&minimal),
        json!({"description":"ALPHA"})
    );
    assert_eq!(serde_json::to_value(candidate).unwrap(), before);
}
#[tokio::test]
async fn indistinguishable_records_remain_unresolved_despite_confident_selection() {
    let mut enricher = enricher();
    let alpha = brand_fixture("alpha", "Alpha Cafe", "", "merchant-studio", "alpha");
    let unrelated = brand_fixture("other", "Alpha Cafe", "", "merchant-studio", "other");
    let rx = mock(&mut enricher, 1, 200, None);
    let (mut results, details) = enricher
        .enrich_batch_candidates_traced(vec![(
            request("Alpha Cafe ON CAN"),
            Ok(vec![alpha, unrelated]),
        )])
        .await;
    assert!(matches!(
        results.remove(0).unwrap().merchant,
        MerchantResult::Unresolved { .. }
    ));
    assert_eq!(details[0]["provider_answer"]["confidence"], 0.99);
    assert_eq!(
        details[0]["provider_answer"]["catalog_resolution"],
        "ambiguous"
    );
    assert!(enricher.store.resolutions(None, 10).unwrap().is_empty());
    assert_eq!(
        rx.recv().unwrap()["questions"]["transaction_0"]["criteria"]
            .as_object()
            .unwrap()
            .len(),
        2
    );
}
#[test]
fn business_grouping_uses_identity_without_source_preferences() {
    let alpha = brand_fixture(
        "alpha",
        "Alpha Cafe",
        "https://alpha.example",
        "merchant-studio",
        "alpha",
    );
    let regional = brand_fixture(
        "regional",
        "ALPHA CAFE",
        "http://www.alpha.example/ca",
        "foursquare",
        "place:regional",
    );
    assert_eq!(
        business_choices(&[alpha.clone(), regional])[0]
            .members
            .len(),
        2
    );
    for (source, external_id, trusted) in [
        ("merchant-studio", "other", false),
        ("open-enrichment", "other", false),
        ("foursquare", "brand:other", false),
        ("manual", "other", true),
    ] {
        let mut other = brand_fixture(
            "other",
            "Alpha Cafe",
            "https://other.example",
            source,
            external_id,
        );
        other.trusted = trusted;
        let choices = business_choices(&[alpha.clone(), other]);
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0].resolved, Some(0));
        assert_eq!(choices[1].resolved, Some(1));
    }
    let plus = brand_fixture(
        "plus",
        "Alpha Cafe+",
        "https://alpha.example",
        "foursquare",
        "place:plus",
    );
    assert_eq!(business_choices(&[alpha, plus]).len(), 2);
}
#[tokio::test]
async fn evaluation_matches_do_not_change_snapshot_or_learn_resolutions() {
    let fixture = enricher();
    let store = fixture.store.clone();
    let before = serde_json::to_value(store.list(None, 10, 0).unwrap()).unwrap();
    let mut evaluator = Enricher::for_evaluation(
        Some("test-key".into()),
        "jev-latest".into(),
        0.95,
        store.clone(),
    )
    .unwrap();
    let rx = mock(&mut evaluator, 1, 200, None);
    let (mut results, evidence) = evaluator
        .enrich_batch_candidates_traced(vec![(
            request("ALPHA CAFE PAYMENT"),
            Ok(candidates(&evaluator, "Alpha Cafe")),
        )])
        .await;
    let result = results.remove(0).unwrap();
    assert_eq!(evidence[0]["provider_answer"]["confidence"], 0.99);
    assert_eq!(evidence[0]["provider_answer"]["choice"], "candidate_0");
    assert_eq!(evidence[0]["method"], "provider");
    assert!(matches!(result.merchant, MerchantResult::Matched { .. }));
    rx.recv().unwrap();
    assert_eq!(
        serde_json::to_value(store.list(None, 10, 0).unwrap()).unwrap(),
        before
    );
    assert!(store.resolutions(None, 10).unwrap().is_empty());
    assert!(store.enrichment_logs(None, None, 10, 0).unwrap().is_empty());
}

#[tokio::test]
async fn abbreviated_name_sends_catalog_city_evidence_to_provider() {
    let mut enricher = enricher();
    let csv = "fsq_place_id,name,country,address,locality,region,latitude,longitude,fsq_category_ids,date_closed\njulius,Julius Cafe,CA,35 John-Savage Rue,Bromont,QC,45.3,-72.6,\"[\"\"restaurant\"\"]\",\n";
    let records = crate::foursquare::prepare(csv, "ca").unwrap().0;
    enricher.store.import(&records).unwrap();
    let rx = mock(&mut enricher, 1, 200, None);
    let (result, details) = enricher
        .enrich_with_details(&request("JULIUS BROMONT"))
        .await;
    assert!(matches!(result.unwrap().merchant,
        MerchantResult::Matched { data } if data.name == "Julius Cafe"));
    assert_eq!(details["method"], "provider");
    let sent = rx.recv().unwrap();
    let evidence =
        &sent["questions"]["transaction_0"]["criteria"]["candidate_0"]["interpretation_evidence"];
    let support = evidence
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["merchant_text"] == "JULIUS")
        .unwrap();
    assert_eq!(support["name_exact"], false);
    assert!(support.get("matched_name").is_none());
    assert_eq!(support["outlet"]["city"], "Bromont");
    assert_eq!(support["outlet"]["country"], "CA");
    assert_eq!(support["outlet"]["source"], "foursquare");
    assert!(support["outlet"].get("external_id").is_none());
    let local_support = details["candidates"][0]["interpretation_evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["merchant_text"] == "JULIUS")
        .unwrap();
    for field in ["license", "attribution", "url"] {
        assert!(
            support["outlet"].get(field).is_none(),
            "provider includes {field}"
        );
        assert!(
            local_support["outlet"][field].as_str().is_some(),
            "audit lost {field}"
        );
    }
}
#[tokio::test]
async fn single_enrichment_details_use_the_actual_provider_shortlist_and_answer() {
    let mut enricher = enricher();
    let rx = mock(&mut enricher, 1, 200, None);
    let (result, details) = enricher
        .enrich_with_details(&request("ALPHA CAFE PAYMENT"))
        .await;
    assert!(matches!(
        result.unwrap().merchant,
        MerchantResult::Matched { .. }
    ));
    assert_eq!(details["method"], "provider");
    assert_eq!(details["status"], "matched");
    assert_eq!(details["interpretation"]["original"], "ALPHA CAFE PAYMENT");
    assert_eq!(details["candidates"][0]["merchant"]["name"], "Alpha Cafe");
    assert_eq!(details["provider_answer"]["choice"], "candidate_0");
    assert_eq!(details["provider_answer"]["confidence"], 0.99);
    let sent = rx.recv().unwrap();
    assert_eq!(details["provider_requests"][0]["body"], sent);
    assert_eq!(details["provider_requests"][0]["status"], "sent");
    let logs = enricher.store.enrichment_logs(None, None, 10, 0).unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0]["data"]["candidates"], details["candidates"]);
    assert_eq!(
        logs[0]["data"]["provider_answer"],
        details["provider_answer"]
    );
}
#[tokio::test]
async fn repeated_branch_provenance_fits_provider_budget_and_preserves_full_results() {
    let mut enricher = enricher();
    let mut shortlisted = candidates(&enricher, "Alpha Cafe");
    let candidate = &mut shortlisted[0];
    candidate.merchant.logo_url = Some("https://example.com/logo.png".into());
    candidate.provenance = (0..1804)
        .map(|index| store::SourceRecord {
            source: "foursquare".into(),
            external_id: format!("branch-{index}"),
            merchant: candidate.merchant.clone(),
            attribution: "Foursquare".into(),
            license: "Apache-2.0".into(),
            url: format!("https://example.com/branch/{index}"),
            version: None,
            raw: json!({"countryHints":["CA"]}),
        })
        .collect();
    let mut distinct = candidate.provenance[0].clone();
    // Repeated source records need one displayed credit, while genuinely
    // different attribution URLs must still be retained.
    for record in candidate.provenance.iter_mut().skip(1) {
        record.url = "https://example.com/branch/1".into();
    }
    distinct.raw = json!({"negativeAliases":["OTHER CAFE"],"transaction_text_regexp":"^ALPHA"});
    candidate.provenance.push(distinct);
    let input = request("Alpha Cafe ON CAN");
    let question = question(&input, &shortlisted);
    assert!(serde_json::to_vec(&question).unwrap().len() < MAX_QUESTION_BYTES);
    let evidence = &question["criteria"]["candidate_0"];
    assert_eq!(evidence["provenance"].as_array().unwrap().len(), 2);
    assert_eq!(
        evidence["provenance"][1]["raw"]["negativeAliases"],
        json!(["OTHER CAFE"])
    );
    assert_eq!(
        evidence["provenance"][1]["raw"]["transaction_text_regexp"],
        "^ALPHA"
    );
    assert!(evidence["merchant"].get("logo_url").is_none());
    assert!(evidence["provenance"][0].get("external_id").is_none());
    let rx = mock(&mut enricher, 1, 200, None);
    let (mut results, details) = enricher
        .enrich_batch_candidates_traced(vec![(input, Ok(shortlisted))])
        .await;
    let response = results.remove(0).unwrap();
    let MerchantResult::Matched { data } = response.merchant else {
        panic!("expected match")
    };
    assert_eq!(
        data.logo_url.as_deref(),
        Some("https://example.com/logo.png")
    );
    assert_eq!(response.attributions.len(), 2);
    assert!(response.attributions[0].contains("https://example.com/branch/0"));
    assert!(response.attributions[1].contains("https://example.com/branch/1"));
    assert_eq!(
        details[0]["candidates"][0]["provenance"]
            .as_array()
            .unwrap()
            .len(),
        1805
    );
    assert_eq!(
        details[0]["provider_requests"][0]["body"],
        rx.recv().unwrap()
    );
}
#[tokio::test]
async fn shared_provider_body_is_recorded_once_per_batch() {
    let mut enricher = enricher();
    let shortlisted = candidates(&enricher, "Alpha Cafe");
    let rx = mock(&mut enricher, 1, 200, None);
    let (results, details) = enricher
        .enrich_batch_candidates_traced(vec![
            (request("Alpha Cafe PURCHASE ONE"), Ok(shortlisted.clone())),
            (request("Alpha Cafe PURCHASE TWO"), Ok(shortlisted)),
        ])
        .await;
    assert!(results.iter().all(Result::is_ok));
    let first = &details[0]["provider_requests"][0];
    let second = &details[1]["provider_requests"][0];
    assert_eq!(first["body"], rx.recv().unwrap());
    assert_eq!(first["request_id"], second["request_id"]);
    assert!(second.get("body").is_none());
    let logs = enricher.store.enrichment_logs(None, None, 10, 0).unwrap();
    assert_eq!(logs.len(), 2);
    let owner = logs
        .iter()
        .find(|l| l["data"]["provider_requests"][0].get("body").is_some())
        .unwrap();
    assert_eq!(second["owner_log_id"], owner["id"]);
    assert_eq!(
        logs.iter()
            .filter(|l| l["data"]["provider_requests"][0].get("body").is_some())
            .count(),
        1
    );
}

#[tokio::test]
async fn provider_merchant_matches_also_enrich_locations() {
    let mut enricher = enricher();
    let rx = mock(&mut enricher, 1, 200, None);
    let candidates = candidates(&enricher, "Alpha Cafe");
    let result = enricher
        .enrich_batch_candidates(vec![(request("Alpha Cafe TORONTO ON CA"), Ok(candidates))])
        .await
        .remove(0)
        .unwrap();
    assert!(matches!(result.merchant, MerchantResult::Matched { .. }));
    let LocationResult::Extracted { data } = result.location else {
        panic!("expected independent geography")
    };
    assert_eq!(data.city.as_deref(), Some("Toronto"));
    rx.recv().unwrap();
}

fn answer(choice: &str) -> Value {
    json!({"type":"choice","choice":choice,"confidence":0.99,"probabilities":{choice:0.99}})
}
fn mock(
    enricher: &mut Enricher,
    count: usize,
    status: u16,
    missing: Option<&'static str>,
) -> mpsc::Receiver<Value> {
    mock_choice(enricher, count, status, missing, "candidate_0")
}
fn mock_choice(
    enricher: &mut Enricher,
    count: usize,
    status: u16,
    missing: Option<&'static str>,
    choice: &'static str,
) -> mpsc::Receiver<Value> {
    mock_answers(enricher, count, status, missing, choice, "transfer")
}
fn mock_answers(
    enricher: &mut Enricher,
    count: usize,
    status: u16,
    missing: Option<&'static str>,
    choice: &'static str,
    kind: &'static str,
) -> mpsc::Receiver<Value> {
    mock_responses(enricher, count, status, move |body| {
        let answers: Map<String, Value> = body["questions"]
            .as_object()
            .unwrap()
            .keys()
            .rev()
            .filter(|key| Some(key.as_str()) != missing)
            .map(|key| {
                (
                    key.clone(),
                    answer(if key.starts_with("kind_") {
                        kind
                    } else {
                        choice
                    }),
                )
            })
            .collect();
        json!({"answers":answers})
    })
}
fn mock_responses(
    enricher: &mut Enricher,
    count: usize,
    status: u16,
    respond: impl Fn(&Value) -> Value + Send + 'static,
) -> mpsc::Receiver<Value> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    enricher.provider_url = format!("http://{}/", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        for _ in 0..count {
            let started = std::time::Instant::now();
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if started.elapsed() > Duration::from_secs(5) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("mock accept failed: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let (header_end, length) = loop {
                let mut buffer = [0; 4096];
                let n = stream.read(&mut buffer).unwrap();
                assert_ne!(n, 0);
                bytes.extend_from_slice(&buffer[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|s| s.trim().parse().unwrap())
                        })
                        .unwrap();
                    break (end + 4, length);
                }
            };
            while bytes.len() < header_end + length {
                let mut buffer = [0; 4096];
                let n = stream.read(&mut buffer).unwrap();
                assert_ne!(n, 0);
                bytes.extend_from_slice(&buffer[..n]);
            }
            let body: Value =
                serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
            let response = respond(&body).to_string();
            write!(stream,"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).unwrap();
            tx.send(body).unwrap();
        }
    });
    rx
}
#[tokio::test]
async fn embedded_names_allow_provider_abstention_in_single_and_batch_requests() {
    let mut enricher = enricher();
    let rx = mock_choice(&mut enricher, 2, 200, None, "none");
    let request = request("TRANSFER TO FRIEND MEMO ALPHA CAFE STORE 1005 2026 10 01");
    let retrieved = enricher
        .store
        .search(&request.description, Some("CA"), 10)
        .unwrap();
    assert_eq!(retrieved[0].merchant.id, "alpha");
    assert!(retrieved.iter().all(|c| !c.exact));
    let single = enricher.enrich(&request).await.unwrap();
    assert!(matches!(single.merchant, MerchantResult::Unresolved { .. }));
    let batch = enricher
        .enrich_batch(std::slice::from_ref(&request))
        .await
        .remove(0)
        .unwrap();
    assert!(matches!(batch.merchant, MerchantResult::Unresolved { .. }));
    for _ in 0..2 {
        let body = rx.recv().unwrap();
        assert!(body.to_string().contains(&request.description));
    }
}
#[tokio::test]
async fn history_survives_reopen_and_retains_unfinished_attempts() {
    let lease = store::MerchantStore::temporary().unwrap();
    let url = lease.temporary_url().to_owned();
    let store = store::MerchantStore::postgres(&url).unwrap();
    let enricher = Enricher::with_store(None, "test-model".into(), 0.95, store.clone()).unwrap();
    enricher
        .start_audit(&[request("interrupted")])
        .await
        .unwrap();
    enricher.enrich(&request("unknown merchant")).await.unwrap();
    drop(enricher);
    drop(store);
    let store = store::MerchantStore::postgres(&url).unwrap();
    let started = store.enrichment_logs(Some("started"), None, 50, 0).unwrap();
    assert_eq!(started.len(), 1);
    assert!(started[0]["finished_at"].is_null());
    assert_eq!(started[0]["data"]["request"]["description"], "interrupted");
    let completed = store
        .enrichment_logs(Some("unresolved"), None, 50, 0)
        .unwrap();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0]["data"]["model"], "test-model");
    assert!(completed[0]["finished_at"].is_string());
    assert_eq!(store.enrichment_logs(None, None, 1, 1).unwrap().len(), 1);
    drop(store);
    drop(lease);
}
#[tokio::test]
async fn batches_isolate_evidence_and_preserve_order_with_local_and_invalid_items() {
    let mut enricher = enricher();
    let alpha = candidates(&enricher, "Alpha Cafe");
    let beta = candidates(&enricher, "Beta Shop");
    let exact = enricher.store.search("Alpha Cafe", Some("CA"), 10).unwrap();
    let rx = mock(&mut enricher, 1, 200, None);
    let results = enricher
        .enrich_batch_candidates(vec![
            (request("alpha transaction"), Ok(alpha)),
            (request(""), Ok(vec![])),
            (request("no candidates"), Ok(vec![])),
            (request("Alpha Cafe"), Ok(exact)),
            (request("beta transaction"), Ok(beta)),
        ])
        .await;
    for (index, id) in [(0, "alpha"), (3, "alpha"), (4, "beta")] {
        assert!(
            matches!(&results[index],Ok(EnrichResponse {merchant:MerchantResult::Matched {data},..}) if data.id == id)
        );
    }
    assert!(results[1].is_err());
    assert!(matches!(
        &results[2],
        Ok(EnrichResponse {
            merchant: MerchantResult::Unresolved { .. },
            ..
        })
    ));
    let logs = enricher.store.enrichment_logs(None, None, 50, 0).unwrap();
    assert_eq!(logs.len(), 5);
    let mut logs = logs;
    logs.sort_by_key(|row| row["data"]["batch_index"].as_u64().unwrap());
    assert!(
        logs.iter()
            .all(|row| row["batch_id"] == logs[0]["batch_id"] && row["finished_at"].is_string())
    );
    assert_eq!(logs[0]["data"]["method"], "provider");
    assert_eq!(logs[0]["data"]["provider_answer"]["confidence"], 0.99);
    assert_eq!(logs[0]["data"]["candidates"][0]["merchant"]["id"], "alpha");
    assert_eq!(logs[1]["status"], "error");
    assert!(logs[1]["data"]["error"].is_string());
    assert_eq!(logs[2]["status"], "unresolved");
    assert_eq!(logs[2]["data"]["method"], "no_candidates");
    assert_eq!(logs[3]["data"]["method"], "exact");
    assert_eq!(
        logs[3]["data"]["kind_provider_answer"]["choice"],
        "transfer"
    );
    assert_eq!(
        enricher
            .store
            .enrichment_logs(Some("matched"), Some("alpha"), 50, 0)
            .unwrap()
            .len(),
        2
    );
    let body = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(body["state"], json!({}));
    let questions = body["questions"].as_object().unwrap();
    assert_eq!(questions.len(), 6);
    assert_eq!(
        questions["transaction_0"]["instructions"]["transaction"]["description"],
        "alpha transaction"
    );
    assert_eq!(
        questions["transaction_4"]["instructions"]["transaction"]["description"],
        "beta transaction"
    );
    assert_eq!(
        questions["transaction_0"]["criteria"]["candidate_0"]["merchant"]["name"],
        "Alpha Cafe"
    );
    assert_eq!(
        questions["transaction_4"]["criteria"]["candidate_0"]["merchant"]["name"],
        "Beta Shop"
    );
}
#[tokio::test]
async fn regex_candidates_reach_provider_with_source_evidence() {
    let csv = "id,name,parent_id,website_url,transaction_text_examples,transaction_text_regexp\nregex-brand,Example Brand,,https://example.com,,(?i)^ZXQ\\b\n";
    let bundle =
        crate::datasets::prepare(crate::datasets::Source::OpenEnrichment, csv, None, "global")
            .unwrap();
    let mut enricher = enricher();
    enricher.store.import(&bundle.records).unwrap();
    let id = enricher
        .store
        .resolve_source("open-enrichment", "regex-brand")
        .unwrap()
        .unwrap();
    let rx = mock(&mut enricher, 1, 200, None);
    let response = enricher.enrich(&request("SQ * ZXQ 9876")).await.unwrap();
    assert!(matches!(response.merchant, MerchantResult::Matched { data } if data.id == id));
    assert!(!response.attributions.is_empty());
    let body = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let candidate = &body["questions"]["transaction_0"]["criteria"]["candidate_0"];
    assert_eq!(candidate["merchant"]["name"], "Example Brand");
    assert_eq!(
        candidate["provenance"][0]["raw"]["transaction_text_regexp"],
        r"(?i)^ZXQ\b"
    );
}
#[test]
fn evaluator_receives_catalog_interpretations_with_unconfirmed_locality() {
    let enricher = enricher();
    let request = request("SQ *Alpha Cafe Bromont 00482");
    let candidates = enricher.store.search_request(&request, 10).unwrap();
    let body = question(&request, &candidates);
    let evidence = &body["criteria"]["candidate_0"]["interpretation_evidence"][0];
    assert!(evidence.get("matched_name").is_none());
    assert_eq!(evidence["possible_location"], "Bromont");
    assert!(evidence["outlet"].is_null());
    assert_eq!(
        body["instructions"]["transaction"]["description"],
        request.description
    );
}
#[tokio::test]
async fn count_and_byte_budgets_split_requests_and_missing_answers_are_isolated() {
    let mut enricher = enricher();
    let candidates = candidates(&enricher, "Alpha Cafe");
    let rx = mock(&mut enricher, 33, 200, Some("transaction_1"));
    let inputs = (0..33)
        .map(|i| (request(&format!("purchase {i}")), Ok(candidates.clone())))
        .collect();
    let results = enricher.enrich_batch_candidates(inputs).await;
    assert_eq!(results.len(), 33);
    assert!(results[1].is_err());
    assert!(results.iter().enumerate().all(|(i, r)| i == 1 || r.is_ok()));
    let mut question_count = 0;
    let mut request_count = 0;
    while question_count < 66 {
        let body = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let count = body["questions"].as_object().unwrap().len();
        assert!(count > 0 && count <= MAX_QUESTIONS);
        assert!(serde_json::to_vec(&body).unwrap().len() <= MAX_BODY_BYTES);
        question_count += count;
        request_count += 1;
    }
    assert_eq!(question_count, 66);
    assert!(request_count > 1);
    let rx = mock(&mut enricher, 3, 200, None);
    let mut large = request("purchase");
    large
        .extra
        .insert("evidence".into(), json!("x".repeat(17 * 1024)));
    let results = enricher
        .enrich_batch_candidates(
            (0..3)
                .map(|_| (large.clone(), Ok(candidates.clone())))
                .collect(),
        )
        .await;
    assert!(results.iter().all(Result::is_ok));
    for _ in 0..3 {
        assert!(
            serde_json::to_vec(&rx.recv_timeout(Duration::from_secs(2)).unwrap())
                .unwrap()
                .len()
                <= MAX_BODY_BYTES
        );
    }
}
#[tokio::test]
async fn upstream_kind_failure_affects_local_results_and_oversized_items_are_rejected() {
    let mut enricher = enricher();
    let candidates = candidates(&enricher, "Alpha Cafe");
    let mut large = request("purchase");
    large
        .extra
        .insert("evidence".into(), json!("x".repeat(MAX_QUESTION_BYTES)));
    let rx = mock(&mut enricher, 1, 429, None);
    let (results, details) = enricher
        .enrich_batch_candidates_traced(vec![
            (request("purchase"), Ok(candidates.clone())),
            (request("empty"), Ok(vec![])),
            (large, Ok(candidates)),
        ])
        .await;
    assert!(results[0].as_ref().unwrap_err().to_string().contains("429"));
    assert!(results[1].as_ref().unwrap_err().to_string().contains("429"));
    assert!(
        results[2]
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("budget")
    );
    assert_eq!(details[0]["provider_requests"][0]["status"], "sent");
    assert_eq!(details[1]["provider_requests"][0]["status"], "sent");
    assert_eq!(details[2]["provider_requests"][0]["status"], "not_sent");
    assert!(
        serde_json::to_vec(
            &details[2]["provider_requests"][0]["body"]["questions"]["transaction_2"]
        )
        .unwrap()
        .len()
            > MAX_QUESTION_BYTES
    );
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap()["questions"]
            .as_object()
            .unwrap()
            .len(),
        3
    );
}
#[tokio::test]
async fn single_transaction_uses_the_same_provider_question_format() {
    let mut enricher = enricher();
    let rx = mock(&mut enricher, 1, 200, None);
    let result = enricher
        .enrich(&request("Alpha Cafe PURCHASE"))
        .await
        .unwrap();
    assert!(matches!(result.merchant, MerchantResult::Matched { .. }));
    let body = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(body["questions"].as_object().unwrap().len(), 2);
    assert_eq!(
        body["questions"]["transaction_0"]["instructions"]["transaction"]["description"],
        "Alpha Cafe PURCHASE"
    );
}

#[tokio::test]
async fn kind_is_classified_for_empty_and_exact_matches_and_missing_answers_are_isolated() {
    let mut enricher = enricher();
    let exact = enricher.store.search("Alpha Cafe", Some("CA"), 10).unwrap();
    let rx = mock(&mut enricher, 1, 200, Some("kind_1"));
    let results = enricher
        .enrich_batch_candidates(vec![
            (request("INTERAC E-TRANSFER TO JANE SMITH"), Ok(vec![])),
            (request("Alpha Cafe"), Ok(exact.clone())),
            (request("Alpha Cafe"), Ok(exact)),
        ])
        .await;
    assert_eq!(results[0].as_ref().unwrap().kind, TransactionKind::Transfer);
    assert!(matches!(
        results[0].as_ref().unwrap().merchant,
        MerchantResult::Unresolved { .. }
    ));
    assert!(
        results[1]
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("kind")
    );
    assert_eq!(results[2].as_ref().unwrap().kind, TransactionKind::Transfer);
    assert!(matches!(
        results[2].as_ref().unwrap().merchant,
        MerchantResult::Matched { .. }
    ));
    let sent = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let questions = sent["questions"].as_object().unwrap();
    assert_eq!(questions.len(), 3);
    assert!(questions.keys().all(|key| key.starts_with("kind_")));
    assert_eq!(
        questions["kind_0"]["instructions"]["transaction"]["description"],
        "INTERAC E-TRANSFER TO JANE SMITH"
    );
}

#[tokio::test]
async fn local_enrichment_without_jev_keeps_kind_unknown() {
    let mut enricher = enricher();
    enricher.api_key = None;
    for description in ["Alpha Cafe", "INTERAC E-TRANSFER TO JANE SMITH"] {
        let result = enricher.enrich(&request(description)).await.unwrap();
        assert_eq!(result.kind, TransactionKind::Unknown);
    }
}

#[tokio::test]
async fn exact_merchant_does_not_force_purchase_kind() {
    let mut enricher = enricher();
    let exact = enricher.store.search("Alpha Cafe", Some("CA"), 10).unwrap();
    let rx = mock_answers(&mut enricher, 1, 200, None, "candidate_0", "refund");
    let result = enricher
        .enrich_batch_candidates(vec![(request("Alpha Cafe"), Ok(exact))])
        .await
        .remove(0)
        .unwrap();
    assert_eq!(result.kind, TransactionKind::Refund);
    assert!(matches!(result.merchant, MerchantResult::Matched { .. }));
    let sent = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(sent["questions"].get("transaction_0").is_none());
    assert!(sent["questions"].get("kind_0").is_some());
}

fn same_name_fixture() -> Vec<store::Candidate> {
    vec![
        brand_fixture(
            "place",
            "Alpha Cafe",
            "https://venue.example",
            "foursquare",
            "one",
        ),
        brand_fixture(
            "alpha",
            "Alpha Cafe",
            "https://alpha.example",
            "foursquare",
            "two",
        ),
        brand_fixture(
            "beta",
            "Beta Shop",
            "https://beta.example",
            "foursquare",
            "three",
        ),
    ]
}

#[tokio::test]
async fn hierarchy_preserves_abstentions_and_isolates_invalid_identity_answers() {
    let mut enricher = enricher();
    let rx = mock_responses(&mut enricher, 2, 200, |body| {
        let answers: Map<_, _> = body["questions"]
            .as_object()
            .unwrap()
            .keys()
            .filter_map(|key| {
                if key == "identity_5" {
                    return None;
                }
                let mut result = answer(if key.starts_with("kind_") {
                    "transfer"
                } else if key == "transaction_1" || key == "identity_2" {
                    "none"
                } else if key == "identity_3" {
                    "candidate_2"
                } else if key.starts_with("identity_") {
                    "candidate_1"
                } else {
                    "candidate_0"
                });
                if key == "transaction_0" || key == "identity_4" {
                    let choice = result["choice"].as_str().unwrap().to_string();
                    result["probabilities"][choice] = json!(0.89);
                }
                Some((key.clone(), result))
            })
            .collect();
        json!({"answers":answers})
    });
    let (results, details) = enricher
        .enrich_batch_candidates_traced(
            (0..7)
                .map(|i| {
                    (
                        request(&format!("Alpha Cafe transaction {i}")),
                        Ok(same_name_fixture()),
                    )
                })
                .collect(),
        )
        .await;
    for index in [0, 1, 2, 4] {
        assert!(matches!(
            &results[index],
            Ok(EnrichResponse {
                merchant: MerchantResult::Unresolved { .. },
                kind: TransactionKind::Transfer,
                ..
            })
        ));
    }
    assert!(
        results[3]
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("outside the supplied question")
    );
    assert!(results[5].is_err());
    assert!(
        matches!(&results[6], Ok(EnrichResponse { merchant: MerchantResult::Matched { data }, .. }) if data.id == "alpha")
    );
    rx.recv().unwrap();
    let followup = rx.recv().unwrap();
    let questions = followup["questions"].as_object().unwrap();
    assert_eq!(questions.len(), 5);
    assert!(!questions.contains_key("identity_0"));
    assert!(!questions.contains_key("identity_1"));
    assert_eq!(details[0]["provider_answer"]["decision_stage"], "name");
    assert_eq!(details[0]["provider_requests"].as_array().unwrap().len(), 1);
    assert_eq!(details[6]["provider_requests"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn hierarchy_rebatches_large_identity_evidence_and_rejects_oversized_questions() {
    let mut enricher = enricher();
    let rx = mock_responses(&mut enricher, 3, 200, |body| {
        let answers: Map<_, _> = body["questions"]
            .as_object()
            .unwrap()
            .keys()
            .map(|key| {
                (
                    key.clone(),
                    answer(if key.starts_with("kind_") {
                        "purchase"
                    } else if key.starts_with("identity_") {
                        "candidate_1"
                    } else {
                        "candidate_0"
                    }),
                )
            })
            .collect();
        json!({"answers":answers})
    });
    let mut records = same_name_fixture();
    // Website evidence is absent from recognition and only sent during identity resolution.
    records[0].merchant.website = Some(format!("https://venue.example/?{}", "x".repeat(16_000)));
    records[0].provenance.clear();
    let mut oversized = records.clone();
    oversized[0].merchant.website = Some(format!("https://venue.example/?{}", "x".repeat(25_000)));
    let (results, details) = enricher
        .enrich_batch_candidates_traced(vec![
            (request("Alpha Cafe first"), Ok(records.clone())),
            (request("Alpha Cafe second"), Ok(records.clone())),
            (request("Alpha Cafe third"), Ok(records)),
            (request("Alpha Cafe fourth"), Ok(oversized)),
        ])
        .await;
    assert!(results[..3].iter().all(|result| matches!(
        result,
        Ok(EnrichResponse {
            merchant: MerchantResult::Matched { .. },
            ..
        })
    )));
    assert!(
        results[3]
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("identity evidence exceeds")
    );
    for _ in 0..3 {
        let sent = rx.recv().unwrap();
        assert!(serde_json::to_vec(&sent).unwrap().len() <= MAX_BODY_BYTES);
        assert!(sent["questions"].as_object().unwrap().len() <= MAX_QUESTIONS);
        for question in sent["questions"].as_object().unwrap().values() {
            assert!(serde_json::to_vec(question).unwrap().len() <= MAX_QUESTION_BYTES);
        }
    }
    assert_eq!(details[3]["provider_requests"][1]["status"], "not_sent");
}

#[tokio::test]
async fn institution_counterparty_survives_provider_matching_and_kind_classification() {
    let bank: Merchant = serde_json::from_value(json!({"id":"bank", "name":"Example Bank",
        "type":"financial_institution", "aliases":["EXAMPLE MONTHLY FEE"]}))
    .unwrap();
    let mut enricher =
        Enricher::new(Some("test-key".into()), "unused".into(), 0.95, vec![bank]).unwrap();
    let candidates = candidates(&enricher, "EXAMPLE MONTHLY FEE");
    let rx = mock_answers(&mut enricher, 1, 200, None, "candidate_0", "fee");
    let (mut results, details) = enricher
        .enrich_batch_candidates_traced(vec![(request("EXAMPLE MONTHLY FEE"), Ok(candidates))])
        .await;
    let result = results.remove(0).unwrap();
    assert_eq!(result.kind, TransactionKind::Fee);
    assert_eq!(result.counterparties[0].id, "bank");
    assert_eq!(
        result.counterparties[0].counterparty_type,
        crate::CounterpartyType::FinancialInstitution
    );
    assert!(matches!(result.merchant, MerchantResult::Unresolved { .. }));
    assert_eq!(details[0]["status"], "matched");
    let sent = rx.recv().unwrap().to_string();
    assert!(sent.contains("financial_institution"));
}

#[path = "identity_live_tests.rs"]
mod identity_live_tests;

#[tokio::test]
async fn identity_refinement_preserves_global_names_and_prefers_country_then_website() {
    for inferred in [false, true] {
        let mut enricher = enricher();
        enricher.persist_matches = false;
        let mut shortlisted = vec![
            brand_fixture(
                "foreign",
                "Alpha Cafe",
                "https://foreign.example",
                "foursquare",
                "foreign",
            ),
            brand_fixture("blank", "Alpha Cafe", "", "foursquare", "blank"),
            brand_fixture(
                "alpha",
                "Alpha Cafe",
                "https://alpha.example",
                "foursquare",
                "alpha",
            ),
            brand_fixture(
                "beta",
                "Beta Shop",
                "https://beta.example",
                "foursquare",
                "beta",
            ),
        ];
        shortlisted[0].merchant.markets = vec!["US".into()];
        shortlisted[3].merchant.markets = vec!["US".into()];
        let rx = mock_responses(&mut enricher, 1, 200, |body| {
            let answers: Map<_, _> = body["questions"]
                .as_object()
                .unwrap()
                .keys()
                .map(|key| {
                    (
                        key.clone(),
                        answer(if key.starts_with("kind_") {
                            "purchase"
                        } else {
                            "candidate_0"
                        }),
                    )
                })
                .collect();
            json!({"answers":answers})
        });
        let mut req = request("Alpha Cafe TORONTO ON");
        if inferred {
            req.country = None;
        }
        let (mut results, details) = enricher
            .enrich_batch_candidates_traced(vec![(req, Ok(shortlisted))])
            .await;
        assert!(
            matches!(results.remove(0).unwrap().merchant, MerchantResult::Matched {data} if data.id == "alpha")
        );
        let sent = rx.recv().unwrap();
        assert_eq!(
            sent["questions"]["transaction_0"]["criteria"]["candidate_3"]["name"],
            "Beta Shop"
        );
        assert_eq!(details[0]["provider_answer"]["identity_country"], "CA");
        assert_eq!(
            details[0]["provider_answer"]["identity_choices"],
            json!(["candidate_2"])
        );
        assert_eq!(
            details[0]["provider_answer"]["identity_method"],
            "deterministic"
        );
    }
}

fn bromont_support(country: Option<&str>) -> crate::interpretation::CatalogSupport {
    crate::interpretation::CatalogSupport {
        merchant_text: "STARBUCKS".into(),
        possible_location: Some("BROMONT".into()),
        location_hint: None,
        geoname_ids: vec![],
        matched_name: "Starbucks".into(),
        name_exact: true,
        outlet: Some(crate::interpretation::OutletSupport {
            source: "fixture".into(),
            external_id: "bromont".into(),
            city: "Bromont".into(),
            country: country.map(str::to_owned),
            attribution: "fixture".into(),
            license: "fixture".into(),
            url: "https://example.test/bromont".into(),
        }),
    }
}

fn bromont_pending() -> Pending {
    let request: EnrichRequest =
        serde_json::from_value(json!({"description":"STARBUCKS BROMONT"})).unwrap();
    let mut candidates: Vec<_> = (0..162).map(|index| {
        let mut candidate = brand_fixture(&format!("starbucks-{index}"), "Starbucks",
            &format!("https://starbucks-{index}.example.test"), "fixture", &index.to_string());
        candidate.merchant.markets = vec!["US".into()];
        candidate.business_evidence = json!({"distinct_catalog_outlets":1, "coverage_is_complete":false,
            "outlets_by_country":{"US":1},"relevant_outlets":[]});
        candidate
    }).collect();
    candidates[0].merchant.markets = vec!["CA".into()];
    candidates[0].interpretation_evidence = vec![bromont_support(Some("CA"))];
    // A global identity remains eligible even without a listed Bromont outlet.
    candidates[1].merchant.markets = vec!["CA".into(), "US".into()];
    let choices = business_choices(&candidates);
    Pending {
        country: None,
        index: 0,
        question: name_question(&request, &candidates, &choices),
        identity_question: Some(identity_question(&request, &candidates)),
        candidates,
        choices,
        kind_question: None,
        local_response: None,
    }
}

#[test]
fn identity_country_requires_exact_agreeing_locality_and_respects_caller_country() {
    let mut pending = bromont_pending();
    let group: Vec<_> = (0..pending.choices.len()).collect();
    assert_eq!(identity_country(&pending, &group).as_deref(), Some("CA"));
    pending.candidates[2].interpretation_evidence = vec![bromont_support(Some("US"))];
    assert_eq!(identity_country(&pending, &group), None);
    pending.country = Some("US".into());
    assert_eq!(identity_country(&pending, &group).as_deref(), Some("US"));
    pending.country = None;
    pending.candidates[2].interpretation_evidence.clear();
    pending.candidates[0].interpretation_evidence[0].name_exact = false;
    assert_eq!(identity_country(&pending, &group), None);
    pending.candidates[0].interpretation_evidence[0].name_exact = true;
    pending.candidates[0].interpretation_evidence[0].possible_location = Some("OTHER CITY".into());
    assert_eq!(identity_country(&pending, &group), None);
    pending.candidates[0].interpretation_evidence = vec![bromont_support(None)];
    assert_eq!(identity_country(&pending, &group), None);
}

#[tokio::test]
async fn starbucks_bromont_narrows_global_identity_payload_without_country_input() {
    let pending = bromont_pending();
    assert!(
        serde_json::to_vec(pending.identity_question.as_ref().unwrap())
            .unwrap()
            .len()
            > MAX_QUESTION_BYTES
    );
    let refined = followup_question(&pending, &answer("candidate_0"), 0.95)
        .unwrap()
        .unwrap();
    assert!(serde_json::to_vec(&refined).unwrap().len() < MAX_QUESTION_BYTES);
    assert_eq!(refined["instructions"]["identity_country"], "CA");
    assert_eq!(refined["criteria"].as_object().unwrap().len(), 3);
    assert!(refined["criteria"].get("candidate_0").is_some());
    assert!(refined["criteria"].get("candidate_1").is_some());
    let merchants = pending
        .candidates
        .iter()
        .map(|c| c.merchant.clone())
        .collect();
    let mut enricher = Enricher::new(
        Some("test-key".into()),
        "jev-latest".into(),
        0.95,
        merchants,
    )
    .unwrap();
    let rx = mock_responses(&mut enricher, 2, 200, |body| {
        let answers: Map<_, _> = body["questions"]
            .as_object()
            .unwrap()
            .keys()
            .map(|key| {
                (
                    key.clone(),
                    answer(if key.starts_with("kind_") {
                        "purchase"
                    } else {
                        "candidate_0"
                    }),
                )
            })
            .collect();
        json!({"answers":answers})
    });
    let request = serde_json::from_value(json!({"description":"STARBUCKS BROMONT"})).unwrap();
    let (results, details) = enricher
        .enrich_batch_candidates_traced(vec![(request, Ok(pending.candidates))])
        .await;
    assert!(
        matches!(&results[0],Ok(EnrichResponse {merchant:MerchantResult::Matched{data},..}) if data.id=="starbucks-0")
    );
    assert_eq!(details[0]["provider_answer"]["identity_country"], "CA");
    assert_eq!(details[0]["provider_requests"].as_array().unwrap().len(), 2);
    for _ in 0..2 {
        let sent = rx.recv().unwrap();
        assert!(serde_json::to_vec(&sent).unwrap().len() <= MAX_BODY_BYTES);
        for question in sent["questions"].as_object().unwrap().values() {
            assert!(serde_json::to_vec(question).unwrap().len() <= MAX_QUESTION_BYTES);
        }
    }
}

#[test]
fn identity_location_filter_prefers_real_outlets_and_falls_back_when_none_match() {
    let mut pending = bromont_pending();
    let refine = |pending: &Pending| {
        followup_question(pending, &answer("candidate_0"), 0.95)
            .unwrap()
            .unwrap()
    };
    let fallback = refine(&pending);
    assert_eq!(
        fallback["instructions"]["outlet_location_filter_applied"],
        false
    );
    assert_eq!(fallback["criteria"].as_object().unwrap().len(), 3);
    // Real outlet support wins even if declared market coverage is incomplete.
    pending.country = Some("CA".into());
    pending.candidates[0].merchant.markets = vec!["US".into()];
    pending.candidates[0].business_evidence["matching_catalog_outlets"] = json!(2);
    let narrowed = refine(&pending);
    assert_eq!(
        narrowed["instructions"]["outlet_location_filter_applied"],
        true
    );
    assert_eq!(narrowed["criteria"].as_object().unwrap().len(), 2);
    assert!(narrowed["criteria"].get("candidate_0").is_some());
    // Two matching identities both remain eligible, including one without a website.
    pending.candidates[1].business_evidence["matching_catalog_outlets"] = json!(1);
    pending.candidates[1].merchant.website = None;
    let ambiguous = refine(&pending);
    assert_eq!(ambiguous["criteria"].as_object().unwrap().len(), 3);
}

#[tokio::test]
async fn bromont_outlet_filter_resolves_merchant_with_multiple_branches_without_identity_model_call()
 {
    let merchants: Vec<Merchant>=serde_json::from_value(json!([
        {"id":"bromont","name":"Starbucks","website":"https://local-starbucks.test","markets":["CA"]},
        {"id":"other","name":"Starbucks","website":"https://other-starbucks.test","markets":["CA"]}
    ])).unwrap();
    let mut enricher = Enricher::new(
        Some("test-key".into()),
        "jev-latest".into(),
        0.95,
        merchants,
    )
    .unwrap();
    let outlets: Vec<crate::location::LocationRecord> = (0..3)
        .map(|index| {
            serde_json::from_value(json!({
                "source":"fixture","external_id":format!("outlet-{index}"),
                "merchant":{"merchant_id":if index<2 {"bromont"} else {"other"}},
                "location":{"city":if index<2 {"Bromont"} else {"Toronto"},"country":"CA",
                    "address":format!("{index} Main St")},
                "attribution":"fixture","license":"fixture","url":"https://example.test"
            }))
            .unwrap()
        })
        .collect();
    enricher.store.import_locations(&outlets).unwrap();
    let rx = mock_responses(&mut enricher, 1, 200, |body| {
        let answers: Map<_, _> = body["questions"]
            .as_object()
            .unwrap()
            .keys()
            .map(|key| {
                (
                    key.clone(),
                    answer(if key.starts_with("kind_") {
                        "purchase"
                    } else {
                        "candidate_0"
                    }),
                )
            })
            .collect();
        json!({"answers":answers})
    });
    let request =
        serde_json::from_value(json!({"description":"STARBUCKS BROMONT","country":"CA"})).unwrap();
    let (result, details) = enricher.enrich_request_traced(&request).await;
    let result = result.unwrap();
    assert!(matches!(result.merchant,MerchantResult::Matched{data} if data.id=="bromont"));
    assert!(
        matches!(result.location, crate::location::LocationResult::Extracted { data }
        if data.city.as_deref() == Some("Bromont") && data.id.is_none())
    );
    assert_eq!(
        details["provider_answer"]["outlet_location_filter_applied"],
        true
    );
    assert_eq!(
        details["provider_answer"]["identity_method"],
        "deterministic"
    );
    assert_eq!(details["provider_requests"].as_array().unwrap().len(), 1);
    rx.recv().unwrap();
}

#[test]
#[ignore = "read-only verification against a full local catalog"]
fn profile_bromont_outlet_filter() -> Result<()> {
    let store = store::MerchantStore::postgres(store::LOCAL_DATABASE_URL)?;
    for country in [None, Some("CA")] {
        let request =
            serde_json::from_value(json!({"description":"STARBUCKS BROMONT","country":country}))?;
        let candidates = store.search_primary_request(&request, 10)?;
        let choices = business_choices(&candidates);
        let pending = Pending {
            country: country.map(str::to_owned),
            index: 0,
            question: name_question(&request, &candidates, &choices),
            identity_question: Some(identity_question(&request, &candidates)),
            candidates,
            choices,
            kind_question: None,
            local_response: None,
        };
        let refined = followup_question(&pending, &answer("candidate_0"), 0.95)?.unwrap();
        assert_eq!(
            refined["instructions"]["outlet_location_filter_applied"],
            true
        );
        for (key, evidence) in refined["criteria"].as_object().unwrap() {
            if key != "none" {
                eprintln!(
                    "{}",
                    json!({"country":country,"choice":key,"website":evidence["merchant"]["website"],
                "matching_outlets":evidence["outlet_summary"]["matching_catalog_outlets"]})
                );
            }
        }
        eprintln!(
            "{}",
            json!({"country":country,"eligible":refined["criteria"].as_object().unwrap().len()-1,"bytes":serde_json::to_vec(&refined)?.len()})
        );
    }
    Ok(())
}
