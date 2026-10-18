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
