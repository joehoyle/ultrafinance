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

pub(crate) fn outlet_summary(request: &EnrichRequest, outlets: &[LocationRecord]) -> Value {
    let interpretation = crate::interpretation::interpret(request);
    let hints: Vec<_> = request
        .location
        .iter()
        .chain(
            interpretation
                .hypotheses
                .iter()
                .filter_map(|h| h.location_hint.as_ref()),
        )
        .collect();
    let descriptor = normalize(&request.description);
    let mut seen = HashSet::new();
    let mut countries = BTreeMap::<String, usize>::new();
    let mut relevant = Vec::new();
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
    json!({"distinct_catalog_outlets":seen.len(),
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
        let summary = outlet_summary(
            &request,
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
}
