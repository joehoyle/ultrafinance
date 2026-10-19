fn processor_fixture() -> Enricher {
    let merchants = serde_json::from_value(json!([
        {"id":"alpha", "name":"Alpha Cafe", "markets":["CA"]},
        {"id":"nexora", "name":"NexoraPay", "aliases":["NX"], "type":"payment_processor", "website":"https://nexora.example"}
    ])).unwrap();
    let enricher = Enricher::new(Some("test-key".into()), "jev-latest".into(), 0.95, merchants).unwrap();
    enricher.store.save_processor_formats(vec![serde_json::from_value(json!({
        "id":"nexora-prefix", "processor_id":"nexora", "pattern":"(?i:NexoraPay|NX)[ \\t]+(?P<merchant>\\S(?:[^\\r\\n]*\\S)?)",
        "sources":["https://nexora.example/format"], "reviewed_on":"2026-10-19", "scope":"Test fixture"
    })).unwrap()], false).unwrap();
    enricher
}

fn merchant_answers(body: &Value) -> Value {
    let answers: Map<_,_> = body["questions"].as_object().unwrap().iter().map(|(key, question)| {
        let choice = if key.starts_with("kind_") { "purchase".to_owned() } else {
            question["criteria"].as_object().unwrap().iter().find(|(key,value)| key.starts_with("candidate_") && value["merchant"]["name"] == "Alpha Cafe").map(|(key,_)| key.clone()).unwrap_or_else(|| "none".into())
        };
        (key.clone(), answer(&choice))
    }).collect();
    json!({"answers":answers})
}

#[tokio::test]
async fn processor_formats_run_first_without_a_processor_provider_request() {
    let mut enricher = processor_fixture();
    let rx = mock_responses(&mut enricher, 1, 200, merchant_answers);
    let original = request("NexoraPay Alpha Caf #0001");
    let (response, details) = enricher.enrich_with_details(&original).await;
    let response = response.unwrap();
    assert_eq!(response.counterparties.iter().map(|party| party.id.as_str()).collect::<Vec<_>>(), vec!["alpha","nexora"]);
    assert_eq!(details["processor_resolution"]["method"], "format");
    assert_eq!(details["processor_resolution"]["format"]["id"], "nexora-prefix");
    assert_eq!(details["merchant_descriptor"]["text"], "Alpha Caf #0001");
    let sent = rx.recv().unwrap();
    assert_eq!(sent["questions"]["transaction_0"]["instructions"]["transaction"]["description"], "Alpha Caf #0001");
    assert_eq!(sent["questions"]["kind_0"]["instructions"]["transaction"]["description"], original.description);
    assert!(!sent.to_string().contains("https://nexora.example"));
    assert!(response.attributions.iter().any(|credit| credit.contains("https://nexora.example/format")));
    assert!(matches!(response.location, LocationResult::Extracted { data } if data.store_number.as_deref() == Some("0001")));
    let logs = enricher.store.enrichment_logs(None,None,10,0).unwrap();
    assert_eq!(logs[0]["data"]["processor_resolution"]["method"], "format");
    assert_eq!(logs[0]["data"]["provider_requests"].as_array().unwrap().len(),1);
}

#[tokio::test]
async fn processor_formats_work_without_credentials_and_do_not_supply_outlets() {
    let mut enricher = processor_fixture();
    enricher.api_key = None;
    let office = serde_json::from_value(json!({
        "source":"processor-fixture", "external_id":"office", "merchant":{"merchant_id":"nexora"},
        "location":{"name":"Processor office", "country":"CA", "store_number":"0001"},
        "attribution":"Processor office fixture", "license":"CC0", "url":"https://nexora.example/office"
    })).unwrap();
    enricher.store.import_locations(&[office]).unwrap();
    let response = enricher.enrich(&request("NX Alpha Cafe")).await.unwrap();
    assert_eq!(response.counterparties.len(),2);
    assert!(matches!(response.location, LocationResult::Unresolved { .. }));
    assert_eq!(response.kind, TransactionKind::Unknown);
    assert!(!response.attributions.iter().any(|credit| credit.contains("Processor office")));
}

#[tokio::test]
async fn processor_formats_leave_unknown_or_disabled_formats_intact() {
    let mut enricher = processor_fixture();
    let mut formats = enricher.store.processor_formats().unwrap(); formats[0].enabled = false;
    enricher.store.save_processor_formats(formats,false).unwrap();
    let rx = mock_responses(&mut enricher,1,200,merchant_answers);
    let original = request("NexoraPay Alpha Cafe");
    let (response, details) = enricher.enrich_with_details(&original).await;
    assert_eq!(response.unwrap().counterparties.len(),1);
    assert_eq!(details["processor_resolution"]["status"],"no_format");
    assert_eq!(details["merchant_descriptor"]["text"], original.description);
    assert_eq!(rx.recv().unwrap()["questions"]["transaction_0"]["instructions"]["transaction"]["description"],original.description);
}

#[tokio::test]
async fn processor_formats_do_not_use_hardcoded_prefixes_as_fallback() {
    let mut enricher = processor_fixture();
    enricher.store.put(&serde_json::from_value(json!({"id":"sumup","name":"SumUp","type":"payment_processor"})).unwrap()).unwrap();
    let rx = mock_responses(&mut enricher,1,200,merchant_answers);
    let original = request("SumUp Alpha Cafe");
    let (response, details) = enricher.enrich_with_details(&original).await;
    assert_eq!(response.unwrap().counterparties.len(),1);
    assert_eq!(details["processor_resolution"]["status"],"no_format");
    assert_eq!(details["merchant_descriptor"]["text"],original.description);
    for hypothesis in details["merchant_interpretation"]["hypotheses"].as_array().unwrap() {
        assert!(hypothesis["merchant_text"].as_str().unwrap().contains("SumUp"));
    }
    assert_eq!(rx.recv().unwrap()["questions"]["transaction_0"]["instructions"]["transaction"]["description"], original.description);
}

#[tokio::test]
async fn processor_formats_fail_closed_on_unseen_overlap() {
    let mut enricher = processor_fixture();
    enricher.store.put(&serde_json::from_value(json!({"id":"other","name":"OtherPay","type":"payment_processor"})).unwrap()).unwrap();
    let mut left = enricher.store.processor_formats().unwrap().remove(0);
    left.id = "route-left".into(); left.pattern = "(?i:ROUTE [AB]) (?P<merchant>.+)".into();
    let mut right = left.clone(); right.id = "route-right".into(); right.processor_id = "other".into();
    right.pattern = "(?i:ROUTE [BC]) (?P<merchant>.+)".into();
    enricher.store.save_processor_formats(vec![left,right],false).unwrap();
    let _rx = mock_responses(&mut enricher,1,200,merchant_answers);
    let (response, details) = enricher.enrich_with_details(&request("ROUTE B Alpha Cafe")).await;
    assert_eq!(response.unwrap().counterparties.len(),1);
    assert_eq!(details["processor_resolution"]["status"],"ambiguous_format");
    assert_eq!(details["merchant_descriptor"]["text"],"ROUTE B Alpha Cafe");
}
