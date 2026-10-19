use super::*;
use crate::{Enricher, MerchantResult, store::MerchantStore};
use serde_json::json;

fn request(description: &str) -> EnrichRequest {
    serde_json::from_value(json!({"description":description})).unwrap()
}
fn records() -> Vec<LocationRecord> {
    serde_json::from_str(include_str!(
        "../../../../../data/locations/open-enrichment-au.json"
    ))
    .unwrap()
}
fn seed(store: &MerchantStore) {
    let records = crate::import::catalog(
        include_str!("../../../../../data/locations/merchants.example.json"),
        "open-enrichment",
    )
    .unwrap();
    store.import(&records).unwrap();
}

#[test]
fn geography_extracts_localities_and_preserves_store_identifiers() {
    let (result, _) = enrich(&request("POLLO TROPICAL #10241 HIALEAH FL"), &[]);
    let LocationResult::Extracted { data } = result else {
        panic!("expected extraction")
    };
    assert_eq!(data.city.as_deref(), Some("Hialeah"));
    assert_eq!(data.region.as_deref(), Some("FL"));
    assert_eq!(data.country.as_deref(), Some("US"));
    assert_eq!(data.store_number.as_deref(), Some("10241"));
    assert_eq!(data.precision, Some(LocationPrecision::City));
    assert!(data.id.is_none() && data.latitude.is_none() && data.longitude.is_none());
    let (result, _) = enrich(&request("WALMART #0006"), &[]);
    let LocationResult::Extracted { data } = result else {
        panic!("expected store identifier")
    };
    assert_eq!(data.store_number.as_deref(), Some("0006"));
    assert_eq!(data.precision, None);
    let (result, _) = enrich(&request("CAFE TORONTO ON CA"), &[]);
    let LocationResult::Extracted { data } = result else {
        panic!("expected extraction")
    };
    assert_eq!(data.country.as_deref(), Some("CA"));
}

#[test]
fn billing_cities_phones_truncation_and_country_hints_do_not_invent_places() {
    for description in [
        "GOOGLE *YOUTUBE MOUNTAIN VIEW CA",
        "OPENAI SAN FRANCISCO CA",
        "PAYPAL *CARVANA 402-935-7733 AZ",
        "HULU LOS ANGELES CA",
        "CAFE CA",
        "LIDL GB NOTTINGHA",
        "ONLINE PURCHASE TORONTO ON",
        "SQ *CAFE 123456",
        "CARD#1234",
        "CARD #1234",
        "AUTH #1234",
        "CAFE #123 #456",
    ] {
        let mut req = request(description);
        req.country = Some("CA".into());
        assert_eq!(
            enrich(&req, &[]).0,
            LocationResult::default(),
            "{description}"
        );
    }
}

#[test]
fn structured_input_is_independent_and_conflicts_do_not_mix_geography() {
    let mut req = request("CAFE HIALEAH FL");
    req.country = Some("CA".into());
    req.location = Some(serde_json::from_value(json!({"city":"Toronto", "country":"CA"})).unwrap());
    let LocationResult::Extracted { data } = enrich(&req, &[]).0 else {
        panic!("expected input evidence")
    };
    assert_eq!(data.city.as_deref(), Some("Toronto"));
    assert_eq!(data.country.as_deref(), Some("CA"));
    assert!(data.region.is_none());
    for hint in [
        json!({"country":"ca"}),
        json!({"city":" "}),
        json!({"store_number":"x".repeat(513)}),
    ] {
        req.location = Some(serde_json::from_value(hint).unwrap());
        assert!(req.validate().is_err());
    }
    assert!(
        serde_json::from_value::<EnrichRequest>(
            json!({"description":"x","location":{"citty":"Toronto"}})
        )
        .is_err()
    );
}

#[test]
fn sole_outlet_matches_without_descriptor_evidence_but_respects_conflicts() {
    let mut outlet = records().remove(0);
    outlet.location.id = Some("loc_only".into());
    let outlets = [outlet];
    let (result, credits) = enrich(&request("MERCHANT PURCHASE"), &outlets);
    assert_eq!(
        result,
        LocationResult::Matched {
            data: outlets[0].location.clone()
        }
    );
    assert_eq!(credits.len(), 1);

    let mut req = request("MERCHANT PURCHASE");
    req.country = Some("CA".into());
    assert!(!matches!(
        enrich(&req, &outlets).0,
        LocationResult::Matched { .. }
    ));
    req.country = Some("AU".into());
    assert!(matches!(
        enrich(&req, &outlets).0,
        LocationResult::Matched { .. }
    ));
    for hint in [
        json!({"city":"Sydney"}),
        json!({"address":"Another street"}),
        json!({"store_number":"R9999"}),
    ] {
        req.location = Some(serde_json::from_value(hint).unwrap());
        assert!(!matches!(
            enrich(&req, &outlets).0,
            LocationResult::Matched { .. }
        ));
    }
    assert!(!matches!(
        enrich(&request("MERCHANT SYDNEY NSW AU"), &outlets).0,
        LocationResult::Matched { .. }
    ));
    // A country filter can reduce a multi-outlet merchant to one branch.
    let mut overseas = outlets[0].clone();
    overseas.location.country = Some("CA".into());
    req.location = None;
    assert!(matches!(
        enrich(&req, &[outlets[0].clone(), overseas]).0,
        LocationResult::Matched { .. }
    ));
}

#[test]
fn geography_filters_outlets_before_selecting_a_unique_branch() {
    let outlets = records();
    for hint in [
        json!({"city":"Canberra"}),
        json!({"region":"ACT"}),
        json!({"postal_code":"2601"}),
        json!({"address":outlets[0].location.address}),
        json!({"store_number":"R483"}),
    ] {
        let mut req = request("MERCHANT PURCHASE");
        req.location = Some(serde_json::from_value(hint).unwrap());
        assert_eq!(
            enrich(&req, &outlets).0,
            LocationResult::Matched {
                data: outlets[0].location.clone()
            }
        );
    }
    assert_eq!(
        enrich(&request("MERCHANT CANBERRA ACT AU"), &outlets).0,
        LocationResult::Matched {
            data: outlets[0].location.clone()
        }
    );
    let mut overseas = outlets[1].clone();
    overseas.location.country = Some("CA".into());
    let choices = [outlets[0].clone(), overseas];
    assert_eq!(
        enrich(&request("MERCHANT AU"), &choices).0,
        LocationResult::Matched {
            data: outlets[0].location.clone()
        }
    );
    assert!(!matches!(
        enrich(&request("MERCHANT US"), &choices).0,
        LocationResult::Matched { .. }
    ));
    assert!(!matches!(
        enrich(&request("MERCHANT PURCHASE"), &choices).0,
        LocationResult::Matched { .. }
    ));
}

#[test]
fn outlets_need_specific_evidence_and_ambiguity_or_conflicts_abstain() {
    let mut outlets = records();
    for (index, record) in outlets.iter_mut().enumerate() {
        record.location.id = Some(format!("loc_{index}"));
    }
    let (result, credits) = enrich(&request("APPLE STORE R483 R483 CANBERRA"), &outlets);
    let LocationResult::Matched { data } = result else {
        panic!("expected outlet")
    };
    assert_eq!(data.id.as_deref(), Some("loc_0"));
    assert_eq!(data.country.as_deref(), Some("AU"));
    assert_eq!(data.precision, Some(LocationPrecision::Outlet));
    assert_eq!(credits.len(), 1);
    assert_eq!(
        enrich(&request("APPLE.COM/BILL SYDNEY"), &outlets).0,
        LocationResult::default()
    );
    assert_eq!(
        enrich(&request("APPLE STORE"), &outlets).0,
        LocationResult::default()
    );
    assert!(!matches!(
        enrich(&request("APPLE STORE R483 SYDNEY NSW AU"), &outlets).0,
        LocationResult::Matched { .. }
    ));
    let mut overseas = request("APPLE STORE R483 R483 CANBERRA");
    overseas.country = Some("CA".into());
    assert!(!matches!(
        enrich(&overseas, &outlets).0,
        LocationResult::Matched { .. }
    ));
    overseas.country = Some("AU".into());
    assert!(matches!(
        enrich(&overseas, &outlets).0,
        LocationResult::Matched { .. }
    ));
    outlets.push(outlets[0].clone());
    assert!(!matches!(
        enrich(&request("APPLE STORE R483"), &outlets).0,
        LocationResult::Matched { .. }
    ));
    let mut req = request("APPLE");
    req.location =
        Some(serde_json::from_value(json!({"store_number":"R238","country":"AU"})).unwrap());
    assert!(matches!(
        enrich(&req, &records()).0,
        LocationResult::Matched { .. }
    ));
}

#[test]
fn imports_are_atomic_stable_and_follow_merchant_links() {
    let store = MerchantStore::temporary().unwrap();
    seed(&store);
    let merchant_id = store
        .resolve_source("open-enrichment", "04eb38e9-d678-4d44-90a9-1786f12effd6")
        .unwrap()
        .unwrap();
    let records = records();
    store.import_locations(&records).unwrap();
    let before = store.locations(&merchant_id).unwrap();
    assert_eq!(before.len(), 2);
    assert!(before[0].location.id.as_ref().unwrap().starts_with("loc_"));
    store.import_locations(&records).unwrap();
    assert_eq!(
        before[0].location.id,
        store.locations(&merchant_id).unwrap()[0].location.id
    );
    let mut bad = records[0].clone();
    bad.external_id = "bad".into();
    bad.merchant = MerchantReference::Local {
        merchant_id: "missing".into(),
    };
    let mut good = records[0].clone();
    good.external_id = "would-be-new".into();
    assert!(store.import_locations(&[good, bad]).is_err());
    assert_eq!(store.locations(&merchant_id).unwrap().len(), 2);
    let mut replacement = store.list(None, 10, 0).unwrap().merchants.remove(0);
    replacement.id = "mer_linked".into();
    store.put(&replacement).unwrap();
    store
        .link(
            "open-enrichment",
            "04eb38e9-d678-4d44-90a9-1786f12effd6",
            "mer_linked",
        )
        .unwrap();
    assert_eq!(store.locations("mer_linked").unwrap().len(), 2);
    assert!(store.locations(&merchant_id).unwrap().is_empty());
}

#[tokio::test]
async fn exact_batch_and_unresolved_merchants_get_independent_locations_and_logs() {
    let store = MerchantStore::temporary().unwrap();
    seed(&store);
    let mut merchant = store.list(None, 10, 0).unwrap().merchants.remove(0);
    merchant
        .aliases
        .push("APPLE STORE R483 R483 CANBERRA".into());
    store.put(&merchant).unwrap();
    store.import_locations(&records()).unwrap();
    let enricher = Enricher::with_store(None, "jev-latest".into(), 0.95, store.clone()).unwrap();
    let results = enricher
        .enrich_batch(&[
            request("APPLE STORE R483 R483 CANBERRA"),
            request("ZXQ #0006 HIALEAH FL"),
            request("ZXQ"),
        ])
        .await;
    assert!(matches!(
        results[0].as_ref().unwrap().merchant,
        MerchantResult::Matched { .. }
    ));
    assert!(matches!(
        results[0].as_ref().unwrap().location,
        LocationResult::Matched { .. }
    ));
    assert!(matches!(
        results[1].as_ref().unwrap().merchant,
        MerchantResult::Unresolved { .. }
    ));
    assert!(matches!(
        results[1].as_ref().unwrap().location,
        LocationResult::Extracted { .. }
    ));
    assert_eq!(
        results[2].as_ref().unwrap().location,
        LocationResult::default()
    );
    let logs = store.enrichment_logs(None, None, 10, 0).unwrap();
    assert!(
        logs.iter()
            .any(|r| r["data"]["response"]["location"]["status"] == "matched")
    );
    let serialized = serde_json::to_value(results[0].as_ref().unwrap()).unwrap();
    let roundtrip: crate::EnrichResponse = serde_json::from_value(serialized).unwrap();
    assert_eq!(roundtrip.location, results[0].as_ref().unwrap().location);
}

#[test]
fn evaluation_measures_fields_outlets_and_abstention_separately() {
    let store = MerchantStore::temporary().unwrap();
    seed(&store);
    store.import_locations(&records()).unwrap();
    let suite: LocationSuite =
        serde_json::from_str(include_str!("../../../../../evals/location-smoke.json")).unwrap();
    let report = evaluate(&store, suite).unwrap();
    assert_eq!(report.geographic_field_accuracy, Some(1.0));
    assert_eq!(report.outlet_accuracy, Some(1.0));
    assert_eq!(report.unresolved_accuracy, Some(1.0));
    let mut suite: LocationSuite =
        serde_json::from_str(include_str!("../../../../../evals/location-smoke.json")).unwrap();
    suite.cases[0].request.description = "ZXQ".into();
    suite.cases[9].request.description = "APPLE STORE R9999".into();
    let report = evaluate(&store, suite).unwrap();
    assert!(report.geographic_field_accuracy.unwrap() < 1.0);
    assert_eq!(report.outlet_accuracy, Some(0.5));
}

#[test]
fn source_refresh_preserves_manual_corrections() {
    let store = MerchantStore::temporary().unwrap();
    seed(&store);
    store.import_locations(&records()).unwrap();
    let mut manual = records().remove(0);
    manual.manual_override = true;
    manual.location.name = Some("Reviewed name".into());
    store.import_locations(&[manual]).unwrap();
    store.import_locations(&records()).unwrap();
    let merchant_id = store
        .resolve_source("open-enrichment", "04eb38e9-d678-4d44-90a9-1786f12effd6")
        .unwrap()
        .unwrap();
    assert!(
        store
            .locations(&merchant_id)
            .unwrap()
            .iter()
            .any(|r| r.location.name.as_deref() == Some("Reviewed name"))
    );
}

#[test]
fn reviewed_record_validation_rejects_bad_geometry_and_patterns() {
    let mut record = records().remove(0);
    record.validate().unwrap();
    record.location.longitude = None;
    assert!(record.validate().is_err());
    record.location.longitude = Some(200.0);
    assert!(record.validate().is_err());
    record.location.longitude = Some(149.0);
    record.transaction_pattern = Some(".*".into());
    assert!(record.validate().is_err());
    record.transaction_pattern = Some("(?=APPLE)".into());
    assert!(record.validate().is_err());
    record.transaction_pattern = None;
    record.aliases = vec!["APPLE".into()];
    record.location.precision = Some(LocationPrecision::City);
    assert!(record.validate().is_err());
}
