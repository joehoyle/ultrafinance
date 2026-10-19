//! Opt-in model regression checks. Synthetic catalog evidence; no learned mappings.
use super::*;

fn catalog(name: &str, websites: &[(&str, &str, usize)]) -> Vec<store::Candidate> {
    websites.iter().map(|&(id, website, count)| {
        let mut candidate = brand_fixture(id, name, website, "fixture", id);
        candidate.business_evidence = json!({"coverage_is_complete":false,
            "distinct_catalog_outlets":count,"outlets_by_country":{"CA":count},"relevant_outlets":[]});
        candidate
    }).collect()
}

#[tokio::test]
#[ignore = "requires TYPESAFE_API_KEY and uses the live Jev provider"]
async fn live_trading_name_controls() {
    let enricher = Enricher::with_store(
        Some(std::env::var("TYPESAFE_API_KEY").expect("set TYPESAFE_API_KEY")),
        std::env::var("JEV_MODEL").unwrap_or_else(|_| "jev-latest".into()),
        0.75,
        store::MerchantStore::temporary().unwrap(),
    )
    .unwrap();
    let names = [
        "Delices",
        "Délices",
        "Les Délices du Mont",
        "Du Mont",
        "Aura Delices",
        "Aux Délices du Boulanger",
        "Aux Délices du Cèdre",
        "Aux Delices du Centre",
        "Akwaba délices",
        "Aux Délices du Cap",
        "Aux Délices du Gourmet",
    ];
    let candidates: Vec<_> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let id = format!("name-{index}");
            brand_fixture(&id, name, &format!("https://{id}.example"), "fixture", &id)
        })
        .collect();
    let mut reversed = candidates.clone();
    reversed.reverse();
    let rona: Vec<_> = ["RONA", "RONA+"]
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let id = format!("rona-{index}");
            brand_fixture(&id, name, &format!("https://{id}.example"), "fixture", &id)
        })
        .collect();
    // Probe recognition itself so unresolved catalog identity cannot hide a
    // failure to recognize the name, or masquerade as a recognition regression.
    for (descriptor, candidates, expected) in [
        ("DELICES DU MONT", &candidates, "Les Délices du Mont"),
        ("DELICES DU MONT", &reversed, "Les Délices du Mont"),
        ("RONA", &rona, "RONA"),
        ("RONA+", &rona, "RONA+"),
    ] {
        let choices = business_choices(candidates);
        let question = name_question(&request(descriptor), candidates, &choices);
        let expected_choice = question["criteria"]
            .as_object()
            .unwrap()
            .iter()
            .find(|(_, evidence)| evidence["name"] == expected)
            .unwrap()
            .0
            .clone();
        let (response, _) = enricher
            .send_provider_request(
                json!({"model":enricher.model,"state":{},"questions":{"name":question}}),
                "recognition",
            )
            .await;
        let response = response.unwrap();
        let answer = &response["answers"]["name"];
        eprintln!("{descriptor}: expected={expected}, answer={answer}");
        assert_eq!(answer["choice"], expected_choice, "{descriptor}");
        assert!(
            answer["probabilities"][&expected_choice].as_f64().unwrap() >= 0.75,
            "{descriptor}: name recognition must pass the existing threshold"
        );
    }
}

#[tokio::test]
#[ignore = "requires TYPESAFE_API_KEY and uses the live Jev provider"]
async fn live_identity_controls() {
    let enricher = Enricher::with_store(
        Some(std::env::var("TYPESAFE_API_KEY").expect("set TYPESAFE_API_KEY")),
        std::env::var("JEV_MODEL").unwrap_or_else(|_| "jev-latest".into()),
        0.90,
        store::MerchantStore::temporary().unwrap(),
    )
    .unwrap();
    let homonyms = catalog(
        "Atlas Cafe",
        &[
            ("atlas-a", "https://atlas-cafe.example", 1),
            ("atlas-b", "https://atlascoffee.example", 1),
        ],
    );
    let mut local = homonyms.clone();
    local[0].business_evidence["distinct_catalog_outlets"] = json!(10000);
    local[0].business_evidence["outlets_by_country"]["CA"] = json!(10000);
    local[1].business_evidence["relevant_outlets"] = json!([{
        "address":"12 Main Street","city":"Bromont","region":"QC","country":"CA","store_number":"43090",
        "matched_fields":["address","city","region","country","store_number"],"conflicting_fields":[],"store_number_in_description":true
    }]);
    let local_request = serde_json::from_value(json!({"description":"ATLAS CAFE 43090 BROMONT QC","country":"CA",
        "location":{"address":"12 Main Street","city":"Bromont","region":"QC","country":"CA","store_number":"43090"}})).unwrap();
    let chain = catalog(
        "Costco",
        &[
            ("chain", "https://www.costco.ca", 100),
            ("unrelated", "https://unrelated-consulting.example", 1),
        ],
    );
    let mut reversed = chain.clone();
    reversed.reverse();
    let cases = [
        ("homonyms", request("ATLAS CAFE"), homonyms.clone(), None),
        (
            "precise outlet beats footprint",
            local_request,
            local,
            Some("atlas-b"),
        ),
        (
            "incidental memo",
            request("INTERAC E-TRANSFER TO JOHN MEMO ATLAS CAFE SUPPLIES"),
            homonyms.clone(),
            None,
        ),
        (
            "unrelated descriptor",
            request("XYZ987 UNKNOWN"),
            homonyms,
            None,
        ),
        (
            "chain identity",
            request("COSTCO WHSE 123 MONTREAL QC"),
            chain,
            Some("chain"),
        ),
        (
            "reordered identities",
            request("COSTCO WHSE 123 MONTREAL QC"),
            reversed,
            Some("chain"),
        ),
    ];
    // process_candidates does not persist aliases, resolutions or audit rows.
    let (results, answers, _) = enricher
        .process_candidates(
            cases
                .iter()
                .map(|(_, request, candidates, _)| (request.clone(), Ok(candidates.clone())))
                .collect(),
        )
        .await;
    for (((label, _, _, expected), result), answer) in cases.iter().zip(results).zip(answers) {
        let result = result.unwrap();
        let actual = match &result.merchant {
            MerchantResult::Matched { data } => Some(data.id.as_str()),
            MerchantResult::Unresolved { .. } => None,
        };
        eprintln!("{label}: expected={expected:?}, actual={actual:?}, answer={answer}");
        assert_eq!(actual, *expected, "{label}");
    }
}
