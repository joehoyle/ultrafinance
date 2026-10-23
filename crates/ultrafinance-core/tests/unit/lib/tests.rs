use super::*;
fn candidates() -> Vec<Merchant> {
    serde_json::from_value(json!([{"id":"mer_1","name":"Example Café","markets":["CA"]}])).unwrap()
}
#[tokio::test]
async fn exact_matches_bypass_jev_but_collisions_and_fuzzy_candidates_do_not() {
    let store = store::MerchantStore::temporary().unwrap();
    let merchant = candidates().remove(0);
    store.put(&merchant).unwrap();
    let request: EnrichRequest =
        serde_json::from_value(json!({"description":"EXAMPLE CAFE","country":"CA"})).unwrap();
    let enricher = Enricher::with_store(None, "jev-latest".into(), 0.95, store.clone()).unwrap();
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
            categories: None,
            category_ids: vec![],
            counterparty_type: Default::default(),
            id: format!("mer_{index}"),
            name: format!("Merchant {index}"),
            markets: vec!["CA".into()],

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
        serde_json::from_value::<EnrichRequest>(json!({"description":"LS","contry":"CA"})).is_err()
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
        MerchantResult::Matched { .. }
    ));
    response["answers"]["merchant"]["probabilities"]["candidate_0"] = json!(0.89);
    response["answers"]["merchant"]["confidence"] = json!(0.99);
    assert!(matches!(
        parse_choice(&response, &candidates, 0.9).unwrap().merchant,
        MerchantResult::Unresolved { .. }
    ));
    response["answers"]["merchant"]["choice"] = json!("candidate_99");
    response["answers"]["merchant"]["probabilities"]["candidate_99"] = json!(0.9);
    assert!(parse_choice(&response, &candidates, 0.9).is_err());
    assert_eq!(
        serde_json::to_value(unresolved()).unwrap(),
        json!({"kind":"unknown","counterparties":[],"merchant":{"status":"unresolved","data":null},"location":{"status":"unresolved","data":null}})
    );
}

#[test]
fn match_cutoff_is_independent_of_choice_count() {
    let candidates = candidates();
    for count in [2, 3, 11, 255] {
        for probability in [0.899, 0.90, 0.91] {
            let confidence = (probability - 1.0 / count as f64) / (1.0 - 1.0 / count as f64);
            let mut probabilities = serde_json::Map::new();
            probabilities.insert("candidate_0".into(), json!(probability));
            probabilities.insert(
                "none".into(),
                json!((1.0 - probability) / (count - 1) as f64),
            );
            for index in 1..count - 1 {
                probabilities.insert(
                    format!("candidate_{index}"),
                    json!((1.0 - probability) / (count - 1) as f64),
                );
            }
            let answer = json!({"type":"choice","choice":"candidate_0", "confidence":confidence,
                "probabilities":probabilities});
            let matched = matches!(
                parse_choice_answer(&answer, &candidates, 0.9)
                    .unwrap()
                    .merchant,
                MerchantResult::Matched { .. }
            );
            assert_eq!(
                matched,
                probability >= 0.9,
                "{count} choices at {probability}"
            );
        }
    }
}
