//! Request-time business evidence. No transaction frequency or learned preferences.
use crate::{EnrichRequest, Merchant, location::LocationRecord, store::normalize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

pub(crate) fn name_key(name: &str) -> String {
    // Retrieval normalization strips meaningful punctuation (e.g. RONA+).
    name.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn identity(merchant: &Merchant) -> Option<(String, String)> {
    let (_, host) = crate::dedupe::deterministic_key(merchant)?;
    Some((
        format!(
            "{}:{}",
            merchant.counterparty_type.as_str(),
            name_key(&merchant.name)
        ),
        host?,
    ))
}

pub(crate) fn outlet_summary(
    request: &EnrichRequest,
    merchant: &Merchant,
    outlets: &[LocationRecord],
) -> Value {
    let interpretation = crate::interpretation::interpret_merchant(request);
    let mut hints: Vec<_> = request
        .location
        .iter()
        .chain(
            interpretation
                .hypotheses
                .iter()
                .filter(|h| {
                    crate::interpretation::supporting_name(merchant, &h.merchant_text).is_some()
                })
                .filter_map(|h| h.location_hint.as_ref()),
        )
        .cloned()
        .collect();
    hints.extend(
        interpretation
            .hypotheses
            .iter()
            .filter(|h| {
                h.location_hint.is_none()
                    && crate::interpretation::supporting_name(merchant, &h.merchant_text).is_some()
            })
            .filter_map(|h| h.possible_location.clone())
            .map(|city| crate::LocationHint {
                city: Some(city),
                ..Default::default()
            }),
    );
    let precise = |hint: &crate::LocationHint| {
        hint.city.is_some()
            || hint.region.is_some()
            || hint.address.is_some()
            || hint.postal_code.is_some()
            || hint.store_number.is_some()
    };
    let explicit = request.location.as_ref().filter(|hint| precise(hint));
    let desired_country = if explicit.is_some() {
        request.country.as_deref().or_else(|| {
            request
                .location
                .as_ref()
                .and_then(|hint| hint.country.as_deref())
        })
    } else {
        crate::store::ranking_country(request, &interpretation)
    };
    let descriptor = normalize(&request.description);
    let mut seen = HashSet::new();
    let mut countries = BTreeMap::<String, usize>::new();
    let mut relevant = Vec::new();
    let mut matching = HashSet::new();
    for outlet in outlets {
        let location = &outlet.location;
        let norm = |s: &Option<String>| s.as_deref().map(normalize).unwrap_or_default();
        let geographic_scope = (
            norm(&location.country),
            norm(&location.region),
            norm(&location.city),
        );
        // Counts require an outlet identifier; city-only source records do not
        // inflate popularity. Repeated source rows at one address count once.
        let identity = if location
            .address
            .as_ref()
            .is_some_and(|a| !a.trim().is_empty())
        {
            Some(json!(["address", geographic_scope, norm(&location.address)]).to_string())
        } else if let (Some(lat), Some(lon)) = (location.latitude, location.longitude) {
            Some(format!("coordinates:{lat:.6}:{lon:.6}"))
        } else if location
            .store_number
            .as_ref()
            .is_some_and(|n| !n.trim().is_empty())
        {
            Some(json!(["store", geographic_scope, norm(&location.store_number)]).to_string())
        } else {
            None
        };
        let matches_hint = |hint: &crate::LocationHint| {
            precise(hint)
                && [
                    (&hint.country, &location.country),
                    (&hint.region, &location.region),
                    (&hint.city, &location.city),
                    (&hint.address, &location.address),
                    (&hint.postal_code, &location.postal_code),
                    (&hint.store_number, &location.store_number),
                ]
                .iter()
                .all(|(expected, actual)| {
                    expected.as_deref().is_none_or(|expected| {
                        actual
                            .as_deref()
                            .is_some_and(|actual| normalize(expected) == normalize(actual))
                    })
                })
        };
        let matches_country = desired_country.is_none_or(|expected| {
            location
                .country
                .as_deref()
                .is_some_and(|actual| expected.eq_ignore_ascii_case(actual))
        });
        if matches_country && explicit.map_or_else(|| hints.iter().any(matches_hint), matches_hint)
        {
            matching.insert(
                identity
                    .clone()
                    .unwrap_or_else(|| json!(["locality", geographic_scope]).to_string()),
            );
        }
        if let Some(identity) = identity
            && seen.insert(identity)
        {
            *countries
                .entry(location.country.clone().unwrap_or_else(|| "unknown".into()))
                .or_default() += 1;
        }
        let mut best = None;
        for hint in &hints {
            let mut matched = Vec::new();
            let mut conflicting = Vec::new();
            for (field, expected, actual) in [
                ("address", &hint.address, &location.address),
                ("city", &hint.city, &location.city),
                ("region", &hint.region, &location.region),
                ("country", &hint.country, &location.country),
                ("postal_code", &hint.postal_code, &location.postal_code),
                ("store_number", &hint.store_number, &location.store_number),
            ] {
                if let (Some(expected), Some(actual)) = (expected, actual) {
                    if !normalize(expected).is_empty() && normalize(expected) == normalize(actual) {
                        matched.push(field);
                    } else if !expected.trim().is_empty() && !actual.trim().is_empty() {
                        conflicting.push(field);
                    }
                }
            }
            // Country/region overlap alone should not dump arbitrary outlets.
            if !matched
                .iter()
                .any(|f| matches!(*f, "address" | "city" | "postal_code" | "store_number"))
            {
                continue;
            }
            let score = matched
                .iter()
                .map(|f| {
                    if matches!(*f, "address" | "store_number" | "postal_code") {
                        4
                    } else {
                        1
                    }
                })
                .sum::<i32>()
                - conflicting.len() as i32 * 4;
            if best
                .as_ref()
                .is_none_or(|(previous, _, _)| score > *previous)
            {
                best = Some((score, matched, conflicting));
            }
        }
        let store_in_description = location.store_number.as_deref().is_some_and(|number| {
            let number = normalize(number);
            !number.is_empty() && descriptor.split_whitespace().any(|token| token == number)
        });
        if best.is_some() || store_in_description {
            let (mut score, matched, mut conflicting) = best.unwrap_or_default();
            if let (Some(expected), Some(actual)) = (&request.country, &location.country)
                && !expected.eq_ignore_ascii_case(actual)
                && !conflicting.contains(&"country")
            {
                conflicting.push("country");
                score -= 4;
            }
            relevant.push((score + if store_in_description {4} else {0}, json!({
                "address":location.address,"city":location.city,"region":location.region,
                "postal_code":location.postal_code,"country":location.country,
                "store_number":location.store_number,"matched_fields":matched,
                "conflicting_fields":conflicting,"store_number_in_description":store_in_description
            })));
        }
    }
    relevant.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_string().cmp(&b.1.to_string()))
    });
    let mut evidence_seen = HashSet::new();
    let relevant: Vec<_> = relevant
        .into_iter()
        .map(|(_, v)| v)
        .filter(|v| evidence_seen.insert(normalize(&v.to_string())))
        .take(3)
        .collect();
    json!({"distinct_catalog_outlets":seen.len(), "matching_catalog_outlets":matching.len(),
        "coverage_is_complete":false,
        "outlets_by_country":countries.into_iter().take(8).collect::<BTreeMap<_,_>>(),
        "relevant_outlets":relevant})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outlet_evidence_deduplicates_counts_and_keeps_conflicts_explicit() {
        let request: EnrichRequest = serde_json::from_value(json!({
            "description":"RONA BROMONT 43090 BROMONT QC", "country":"CA",
            "location":{"city":"Bromont","region":"QC","country":"CA"}
        }))
        .unwrap();
        let outlet = |id: &str, address: Option<&str>, country: &str| -> LocationRecord {
            serde_json::from_value(json!({"source":"fixture","external_id":id,
                "merchant":{"merchant_id":"rona"},"location":{
                    "address":address,"city":"Bromont","region":"QC","country":country,
                    "store_number":if address.is_some(){Some("43090")}else{None}},
                "attribution":"fixture","license":"fixture","url":"https://example.com"
            }))
            .unwrap()
        };
        let merchant = serde_json::from_value(json!({"id":"rona","name":"Rona"})).unwrap();
        let summary = outlet_summary(
            &request,
            &merchant,
            &[
                outlet("one", Some("1 Main St"), "CA"),
                outlet("duplicate", Some("1 MAIN ST"), "CA"),
                outlet("other-country", Some("2 Main St"), "US"),
                outlet("city-only", None, "CA"),
            ],
        );
        assert_eq!(summary["distinct_catalog_outlets"], 2);
        assert_eq!(summary["outlets_by_country"]["CA"], 1);
        assert_eq!(summary["coverage_is_complete"], false);
        let relevant = summary["relevant_outlets"].as_array().unwrap();
        assert!(relevant.len() <= 3);
        assert!(
            relevant
                .iter()
                .any(|v| v["store_number_in_description"] == true && v["country"] == "CA")
        );
        assert!(relevant.iter().any(|v| {
            v["country"] == "US"
                && v["conflicting_fields"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("country"))
        }));
    }
    fn location_fixture(
        id: &str,
        city: &str,
        country: &str,
        address: &str,
        store: &str,
    ) -> LocationRecord {
        serde_json::from_value(json!({"source":"fixture","external_id":id,
            "merchant":{"merchant_id":"starbucks"},"location":{
                "city":city,"country":country,"address":address,"store_number":store},
            "attribution":"fixture","license":"fixture","url":"https://example.test"}))
        .unwrap()
    }

    #[test]
    fn descriptor_location_counts_multiple_outlets_and_respects_country() {
        let merchant =
            serde_json::from_value(json!({"id":"starbucks","name":"Starbucks"})).unwrap();
        let outlets = vec![
            location_fixture("one", "Bromont", "CA", "1 Main St", "001"),
            location_fixture("duplicate", "BROMONT", "CA", "1 MAIN ST", "001"),
            location_fixture("two", "Bromont", "CA", "2 Main St", "002"),
            location_fixture("other-country", "Bromont", "US", "3 Main St", "003"),
            location_fixture("other-city", "Toronto", "CA", "4 Main St", "004"),
        ];
        let request =
            serde_json::from_value(json!({"description":"STARBUCKS BROMONT","country":"CA"}))
                .unwrap();
        assert_eq!(
            outlet_summary(&request, &merchant, &outlets)["matching_catalog_outlets"],
            2
        );
        let request = serde_json::from_value(json!({"description":"STARBUCKS BROMONT"})).unwrap();
        assert_eq!(
            outlet_summary(&request, &merchant, &outlets)["matching_catalog_outlets"],
            3
        );
        let request =
            serde_json::from_value(json!({"description":"STARBUCKS","country":"CA"})).unwrap();
        assert_eq!(
            outlet_summary(&request, &merchant, &outlets)["matching_catalog_outlets"],
            0
        );
        let unrelated = serde_json::from_value(json!({"id":"other","name":"Other Cafe"})).unwrap();
        let request = serde_json::from_value(json!({"description":"STARBUCKS BROMONT"})).unwrap();
        assert_eq!(
            outlet_summary(&request, &unrelated, &outlets)["matching_catalog_outlets"],
            0
        );
    }

    #[test]
    fn explicit_location_overrides_descriptor_city_and_requires_all_supplied_fields() {
        let merchant =
            serde_json::from_value(json!({"id":"starbucks","name":"Starbucks"})).unwrap();
        let outlets = vec![
            location_fixture("descriptor", "Bromont", "CA", "1 Main St", "001"),
            location_fixture("explicit", "Toronto", "CA", "2 Main St", "002"),
            location_fixture("wrong-address", "Toronto", "CA", "3 Main St", "002"),
            location_fixture("wrong-country", "Toronto", "US", "2 Main St", "002"),
            location_fixture("wrong-store", "Toronto", "CA", "2 Main St", "2"),
        ];
        let request =
            serde_json::from_value(json!({"description":"STARBUCKS BROMONT","country":"CA",
            "location":{"city":"Toronto","address":"2 Main St","store_number":"002"}}))
            .unwrap();
        assert_eq!(
            outlet_summary(&request, &merchant, &outlets)["matching_catalog_outlets"],
            1
        );
    }
}
