use super::*;
use std::io::{Read, Write};
use std::sync::mpsc;

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
async fn duplicate_place_names_use_one_brand_choice_and_resolve_the_brand_record() {
    let mut enricher = enricher();
    let place = brand_fixture(
        "place",
        "Alpha Cafe",
        "https://venue.example",
        "foursquare",
        "place:one",
    );
    let brand = brand_fixture(
        "alpha",
        "Alpha Cafe",
        "https://alpha.example",
        "open-enrichment",
        "alpha",
    );
    let beta = brand_fixture(
        "beta",
        "Beta Shop",
        "https://beta.example",
        "merchant-studio",
        "beta",
    );
    let shortlisted = vec![place, brand, beta];
    let rx = mock_choice(&mut enricher, 1, 200, None, "candidate_1");
    let (mut results, details) = enricher
        .enrich_batch_candidates_traced(vec![(
            request("Beta Shop ON CAN"),
            Ok(shortlisted.clone()),
        )])
        .await;
    assert!(
        matches!(results.remove(0).unwrap().merchant, MerchantResult::Matched { data } if data.id == "beta")
    );
    let sent = rx.recv().unwrap();
    let choices = &sent["questions"]["transaction_0"]["criteria"];
    assert_eq!(choices.as_object().unwrap().len(), 3);
    assert_eq!(choices["candidate_0"]["merchant"]["name"], "Alpha Cafe");
    assert_eq!(
        choices["candidate_0"]["merchant"]["website"],
        "https://alpha.example"
    );
    assert_eq!(
        choices["candidate_0"]["provenance"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(choices["candidate_1"]["merchant"]["name"], "Beta Shop");
    assert_eq!(
        details[0]["provider_choices"][0]["record_ids"],
        json!(["place", "alpha"])
    );
    assert_eq!(details[0]["provider_choices"][1]["merchant"]["id"], "beta");
    assert_eq!(
        details[0]["provider_answer"]["catalog_resolution"],
        "resolved"
    );
    assert!(enricher.store.get("place").unwrap().is_none());
    let rx = mock(&mut enricher, 1, 200, None);
    let (mut results, details) = enricher
        .enrich_batch_candidates_traced(vec![(request("Alpha Cafe ON CAN"), Ok(shortlisted))])
        .await;
    assert!(
        matches!(results.remove(0).unwrap().merchant, MerchantResult::Matched { data } if data.id == "alpha")
    );
    assert_eq!(
        details[0]["provider_answer"]["catalog_resolution"],
        "resolved"
    );
    rx.recv().unwrap();
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
        json!({"name":"Alpha Cafe",
        "aliases":["Alpha"],"markets":["CA"],"website":"https://alpha.example"})
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
    let variants = primary["catalog_identity_variants"].as_array().unwrap();
    assert_eq!(variants.len(), 1);
    assert_eq!(variants[0]["merchant"]["website"], "https://other.example");
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
async fn same_name_unrelated_businesses_remain_unresolved_despite_confident_name_selection() {
    let mut enricher = enricher();
    let alpha = brand_fixture(
        "alpha",
        "Alpha Cafe",
        "https://alpha.example",
        "merchant-studio",
        "alpha",
    );
    let unrelated = brand_fixture(
        "other",
        "Alpha Cafe",
        "https://unrelated.example",
        "merchant-studio",
        "other",
    );
    let rx = mock(&mut enricher, 2, 200, None);
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
    assert_eq!(
        details[0]["provider_answer"]["identity_answer"]["choice"],
        "none"
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
#[tokio::test]
async fn country_narrows_only_the_selected_name_and_preserves_global_names() {
    for description in ["Alpha Cafe", "Alpha Cafe TORONTO ON"] {
        let mut enricher = enricher();
        enricher.persist_matches = false;
        let ca = brand_fixture(
            "ca",
            "Alpha Cafe",
            "https://ca.example",
            "foursquare",
            "place:ca",
        );
        let mut us = brand_fixture(
            "us",
            "Alpha Cafe",
            "https://us.example",
            "foursquare",
            "place:us",
        );
        us.merchant.markets = vec!["US".into()];
        let mut foreign_name = brand_fixture(
            "foreign",
            "Beta Shop",
            "https://beta.example",
            "foursquare",
            "place:beta",
        );
        foreign_name.merchant.markets = vec!["US".into()];
        let mut transaction = request(description);
        if description.contains("TORONTO") {
            transaction.country = None;
        }
        let rx = mock(&mut enricher, 1, 200, None);
        let (mut results, details) = enricher
            .enrich_batch_candidates_traced(vec![(transaction, Ok(vec![us, ca, foreign_name]))])
            .await;
        assert!(
            matches!(results.remove(0).unwrap().merchant, MerchantResult::Matched { data } if data.id == "ca")
        );
        let sent = rx.recv().unwrap();
        assert_eq!(
            sent["questions"]["transaction_0"]["criteria"]["candidate_1"]["merchant"]["name"],
            "Beta Shop"
        );
        assert_eq!(
            details[0]["provider_answer"]["identity_candidates"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            details[0]["provider_answer"]["identity_method"],
            "deterministic"
        );
    }
}

#[tokio::test]
async fn second_identity_decision_selects_and_traces_the_actual_record() {
    let mut enricher = enricher();
    enricher.persist_matches = false;
    let records = vec![
        brand_fixture("a", "Alpha", "https://a.example", "foursquare", "place:a"),
        brand_fixture("b", "Alpha", "https://b.example", "foursquare", "place:b"),
    ];
    let rx = mock_identity_choice(&mut enricher, 2, 200, None, "candidate_0", "candidate_1");
    let (mut results, details) = enricher
        .enrich_batch_candidates_traced(vec![(request("Alpha"), Ok(records))])
        .await;
    assert!(
        matches!(results.remove(0).unwrap().merchant, MerchantResult::Matched { data } if data.id == "b")
    );
    let answer = &details[0]["provider_answer"];
    assert_eq!(answer["choice"], "candidate_0");
    assert_eq!(answer["identity_answer"]["choice"], "candidate_1");
    assert_eq!(answer["catalog_resolution"], "resolved");
    assert_eq!(details[0]["provider_requests"].as_array().unwrap().len(), 2);
    assert_eq!(
        rx.recv().unwrap()["questions"]["transaction_0"]["criteria"]
            .as_object()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        rx.recv().unwrap()["questions"]["identity"]["criteria"]
            .as_object()
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn identity_provider_selection_threshold_and_errors_are_enforced() {
    for (selected, threshold, status, missing, expected) in [
        ("candidate_1", 0.95, 200, None, "matched"),
        ("candidate_1", 1.0, 200, None, "unresolved"),
        ("none", 0.95, 200, None, "unresolved"),
        ("candidate_99", 0.95, 200, None, "error"),
        ("candidate_1", 0.95, 500, None, "error"),
        ("candidate_1", 0.95, 200, Some("identity"), "error"),
    ] {
        let mut enricher = enricher();
        enricher.threshold = threshold;
        let records = vec![
            brand_fixture("a", "Alpha", "https://a.example", "foursquare", "place:a"),
            brand_fixture("b", "Alpha", "https://b.example", "foursquare", "place:b"),
        ];
        let rx = mock_identity_choice(&mut enricher, 1, status, missing, "candidate_0", selected);
        let (result, _, _) = enricher
            .evaluate_identity(&request("Alpha"), &records)
            .await;
        match expected {
            "matched" => assert!(
                matches!(result.unwrap().merchant, MerchantResult::Matched { data } if data.id == "b")
            ),
            "unresolved" => assert!(matches!(
                result.unwrap().merchant,
                MerchantResult::Unresolved { .. }
            )),
            _ => assert!(result.is_err()),
        }
        assert!(rx.recv().unwrap()["questions"]["identity"]["criteria"]["none"].is_string());
    }
}

#[test]
fn identity_country_falls_back_within_selected_name_only() {
    let mut first = brand_fixture("a", "Alpha", "https://a.example", "foursquare", "place:a");
    first.merchant.markets = vec!["US".into()];
    let mut second = first.clone();
    second.merchant.id = "b".into();
    second.merchant.markets.clear();
    let other = brand_fixture(
        "other",
        "Beta",
        "https://b.example",
        "foursquare",
        "place:b",
    );
    let candidates = vec![first, second, other];
    let choices = brand_choices(&candidates);
    assert_eq!(
        identity_records(&candidates, &choices[0], Some("CA")).len(),
        2
    );
    assert_eq!(
        identity_records(&candidates, &choices[0], Some("US")).len(),
        1
    );
}

#[test]
fn identity_websites_refine_only_the_country_filtered_group_with_fallback() {
    let linked = brand_fixture(
        "linked",
        "Alpha",
        "https://alpha.example",
        "foursquare",
        "place:a",
    );
    let mut missing = linked.clone();
    missing.merchant.id = "missing".into();
    missing.merchant.website = None;
    let mut blank = linked.clone();
    blank.merchant.id = "blank".into();
    blank.merchant.website = Some("  ".into());
    let mut foreign = linked.clone();
    foreign.merchant.id = "foreign".into();
    foreign.merchant.markets = vec!["US".into()];
    let mut other = linked.clone();
    other.merchant.id = "other".into();
    other.merchant.name = "Beta".into();
    let candidates = vec![
        missing.clone(),
        linked,
        blank.clone(),
        foreign.clone(),
        other.clone(),
    ];
    let choices = brand_choices(&candidates);
    let refined = identity_records(&candidates, &choices[0], Some("CA"));
    assert_eq!(
        refined
            .iter()
            .map(|c| c.merchant.id.as_str())
            .collect::<Vec<_>>(),
        vec!["linked"]
    );
    let candidates = vec![missing, blank, foreign, other];
    let choices = brand_choices(&candidates);
    let refined = identity_records(&candidates, &choices[0], Some("CA"));
    assert_eq!(
        refined
            .iter()
            .map(|c| c.merchant.id.as_str())
            .collect::<Vec<_>>(),
        vec!["missing", "blank"]
    );
    let refined = identity_records(&candidates, &choices[0], Some("GB"));
    assert_eq!(
        refined
            .iter()
            .map(|c| c.merchant.id.as_str())
            .collect::<Vec<_>>(),
        vec!["foreign"]
    );
}

#[test]
fn catalog_resolution_requires_identity_evidence_and_preserves_distinct_names() {
    let brand = brand_fixture(
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
        "merchant-studio",
        "regional",
    );
    assert_eq!(
        brand_choices(&[brand.clone(), regional])[0].resolved,
        Some(0)
    );
    let place = brand_fixture(
        "place",
        "Alpha Cafe",
        "https://venue.example",
        "foursquare",
        "place:one",
    );
    assert_eq!(
        brand_choices(&[place.clone(), brand.clone()])[0].resolved,
        Some(1)
    );
    let other_place = brand_fixture(
        "other",
        "Alpha Cafe",
        "https://other.example",
        "foursquare",
        "place:two",
    );
    assert_eq!(brand_choices(&[place, other_place])[0].resolved, None);
    let mut child = brand_fixture(
        "child",
        "Alpha Cafe",
        "https://child.example",
        "open-enrichment",
        "child",
    );
    child.provenance[0].raw = json!({"parent_id":"parent"});
    assert!(!established_brand(&child));
    assert_eq!(brand_choices(&[brand.clone(), child])[0].resolved, None);
    let distinct = brand_fixture(
        "distinct",
        "Alpha Cafe Plus",
        "https://alpha.example",
        "merchant-studio",
        "plus",
    );
    assert_eq!(brand_choices(&[brand, distinct]).len(), 2);
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
        .enrich_with_details(&request("SQ* JULIUS BROMONT"))
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
    mock_identity_choice(enricher, count, status, missing, choice, "none")
}
fn mock_identity_choice(
    enricher: &mut Enricher,
    count: usize,
    status: u16,
    missing: Option<&'static str>,
    choice: &'static str,
    identity_choice: &'static str,
) -> mpsc::Receiver<Value> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    enricher.provider_url = format!("http://{}/", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..count {
            let (mut stream, _) = listener.accept().unwrap();
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
            let answers: Map<String, Value> = body["questions"]
                .as_object()
                .unwrap()
                .keys()
                .rev()
                .filter(|key| Some(key.as_str()) != missing)
                .map(|key| {
                    (
                        key.clone(),
                        answer(if key == "identity" {
                            identity_choice
                        } else {
                            choice
                        }),
                    )
                })
                .collect();
            let response = json!({"answers":answers}).to_string();
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
    assert!(logs[3]["data"]["provider_answer"].is_null());
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
    assert_eq!(questions.len(), 2);
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
    let rx = mock(&mut enricher, 2, 200, Some("transaction_1"));
    let inputs = (0..33)
        .map(|i| (request(&format!("purchase {i}")), Ok(candidates.clone())))
        .collect();
    let results = enricher.enrich_batch_candidates(inputs).await;
    assert_eq!(results.len(), 33);
    assert!(results[1].is_err());
    assert!(results.iter().enumerate().all(|(i, r)| i == 1 || r.is_ok()));
    for _ in 0..2 {
        let body = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(body["questions"].as_object().unwrap().len() <= MAX_QUESTIONS);
        assert!(serde_json::to_vec(&body).unwrap().len() <= MAX_BODY_BYTES);
    }
    let rx = mock(&mut enricher, 2, 200, None);
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
    for _ in 0..2 {
        assert!(
            serde_json::to_vec(&rx.recv_timeout(Duration::from_secs(2)).unwrap())
                .unwrap()
                .len()
                <= MAX_BODY_BYTES
        );
    }
}
#[tokio::test]
async fn upstream_failure_preserves_local_results_and_oversized_items_are_rejected() {
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
    assert!(results[1].is_ok());
    assert!(
        results[2]
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("budget")
    );
    assert_eq!(details[0]["provider_requests"][0]["status"], "sent");
    assert!(details[1].get("provider_requests").is_none());
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
        1
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
    assert_eq!(body["questions"].as_object().unwrap().len(), 1);
    assert_eq!(
        body["questions"]["transaction_0"]["instructions"]["transaction"]["description"],
        "Alpha Cafe PURCHASE"
    );
}
