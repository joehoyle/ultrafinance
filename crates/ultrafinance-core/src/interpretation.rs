//! Bounded hypotheses copied from the descriptor, never verified business facts.

pub use crate::regex_rules::ProcessorFormat;
use crate::{EnrichRequest, store::normalize};
use serde::Serialize;
#[path = "bank_formats.rs"]
mod bank_formats;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Format {
    pub id: &'static str,
    pub description: &'static str,
    pub pattern: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct Formats {
    pub formats: Vec<Format>,
    pub processors: &'static [ProcessorFormat],
    pub gazetteer: &'static serde_json::Value,
}

const NAME_RULES: &[Format] = &[
    Format {
        id: "terminal-numbers",
        description: "Remove terminal numeric tokens of at least three digits, optionally prefixed by #; embedded numbers survive.",
        pattern: Some(r"^#*[0-9]{3,}$"),
    },
    Format {
        id: "trailing-localities",
        description: "Keep the full name and add possible one- or two-word trailing localities; each locality word must contain a letter.",
        pattern: Some(r"\p{Alphabetic}"),
    },
    Format {
        id: "slash-localities",
        description: "Add a possible locality for Merchant / City / Country with a two- or three-letter country token.",
        pattern: Some(r"^[A-Za-z]{2,3}$"),
    },
    Format {
        id: "original-text",
        description: "Always preserve original text; retain an original-name hypothesis when cleaning changes the normalized name. Parsed matches remain unverified.",
        pattern: None,
    },
];

fn name_pattern(id: &str) -> &'static regex::Regex {
    static PATTERNS: std::sync::OnceLock<Vec<(&str, regex::Regex)>> = std::sync::OnceLock::new();
    PATTERNS
        .get_or_init(|| {
            NAME_RULES
                .iter()
                .filter_map(|rule| {
                    rule.pattern
                        .map(|pattern| (rule.id, regex::Regex::new(pattern).unwrap()))
                })
                .collect()
        })
        .iter()
        .find(|(key, _)| *key == id)
        .map(|(_, regex)| regex)
        .unwrap()
}

/// Inventory generated from the definitions used by descriptor parsing.
pub fn formats() -> Formats {
    Formats {
        formats: bank_formats::formats()
            .chain(NAME_RULES.iter().copied())
            .chain(crate::descriptor_location::formats())
            .collect(),
        processors: crate::regex_rules::processor_formats(),
        gazetteer: crate::gazetteer::metadata(),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Hypothesis {
    pub merchant_text: String,
    pub possible_location: Option<String>,
    /// Catalog-validated descriptor clues, never verified purchase geography.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location_hint: Option<crate::LocationHint>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub geoname_ids: Vec<u32>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Interpretation {
    pub original: String,
    pub processor_hint: Option<String>,
    pub unverified_tokens: Vec<String>,
    pub hypotheses: Vec<Hypothesis>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogSupport {
    pub merchant_text: String,
    pub possible_location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location_hint: Option<crate::LocationHint>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub geoname_ids: Vec<u32>,
    pub matched_name: String,
    pub name_exact: bool,
    pub outlet: Option<OutletSupport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutletSupport {
    pub source: String,
    pub external_id: String,
    pub city: String,
    pub country: Option<String>,
    pub attribution: String,
    pub license: String,
    pub url: String,
}

/// Partial names must account for every descriptor name token; shared geography
/// alone must not promote unrelated businesses from the retrieval pool.
pub(crate) fn supporting_name<'a>(merchant: &'a crate::Merchant, text: &str) -> Option<&'a String> {
    let query = normalize(text);
    std::iter::once(&merchant.name)
        .chain(&merchant.aliases)
        .find(|name| {
            let known = normalize(name);
            !query.is_empty()
                && (query == known
                    || query.split_whitespace().all(|token| {
                        known.split_whitespace().any(|word| {
                            token == word
                                || token.chars().count() >= 4
                                    && word.chars().count() >= 4
                                    && rapidfuzz::fuzz::ratio(token.chars(), word.chars()) >= 0.8
                        })
                    }))
        })
}

/// Name equality or retrieved-name plus stored city agreement support an
/// interpretation, not trust. Fuzzy names require catalog location evidence.
/// No geography is inferred from a suffix alone, or written back to the catalog.
pub(crate) fn catalog_support(
    interpretation: &Interpretation,
    merchant: &crate::Merchant,
    outlets: &[crate::location::LocationRecord],
    country: Option<&str>,
) -> Vec<CatalogSupport> {
    interpretation
        .hypotheses
        .iter()
        .filter_map(|hypothesis| {
            let name = supporting_name(merchant, &hypothesis.merchant_text)?;
            let exact_name = std::iter::once(&merchant.name)
                .chain(&merchant.aliases)
                .find(|name| normalize(name) == normalize(&hypothesis.merchant_text));
            let mut matching = outlets.iter().filter(|record| {
                if record.location.city.is_none()
                    || country
                        .is_some_and(|country| record.location.country.as_deref() != Some(country))
                {
                    return false;
                }
                if let Some(hint) = &hypothesis.location_hint {
                    let same_field = |clue: &Option<String>, known: &Option<String>| {
                        clue.as_ref().is_none_or(|clue| {
                            known
                                .as_ref()
                                .is_some_and(|known| normalize(clue) == normalize(known))
                        })
                    };
                    if !same_field(&hint.country, &record.location.country)
                        || !same_field(&hint.store_number, &record.location.store_number)
                    {
                        return false;
                    }
                    if hypothesis.geoname_ids.is_empty() {
                        same_field(&hint.city, &record.location.city)
                            && same_field(&hint.region, &record.location.region)
                    } else {
                        (hint.region.is_none() || record.location.region.is_some())
                            && crate::gazetteer::lookup(
                                record.location.city.as_deref().unwrap(),
                                record.location.country.as_deref(),
                                record.location.region.as_deref(),
                            )
                            .iter()
                            .any(|city| hypothesis.geoname_ids.contains(&city.geoname_id))
                    }
                } else {
                    hypothesis.possible_location.as_ref().is_some_and(|city| {
                        record
                            .location
                            .city
                            .as_ref()
                            .is_some_and(|known| normalize(known) == normalize(city))
                    })
                }
            });
            let first = matching.next();
            // City-level agreement cannot pick one of several outlets in that city.
            let outlet = first
                .filter(|_| matching.next().is_none())
                .map(|record| OutletSupport {
                    source: record.source.clone(),
                    external_id: record.external_id.clone(),
                    city: record.location.city.clone().unwrap(),
                    country: record.location.country.clone(),
                    attribution: record.attribution.clone(),
                    license: record.license.clone(),
                    url: record.url.clone(),
                });
            // Retrieved fuzzy names are candidates, not confirmed aliases.
            // Include them only with independent, unambiguous location support.
            if exact_name.is_none() && outlet.is_none() {
                return None;
            }
            Some(CatalogSupport {
                merchant_text: hypothesis.merchant_text.clone(),
                possible_location: hypothesis.possible_location.clone(),
                location_hint: hypothesis.location_hint.clone(),
                geoname_ids: hypothesis.geoname_ids.clone(),
                matched_name: exact_name.unwrap_or(name).clone(),
                name_exact: exact_name.is_some(),
                outlet,
            })
        })
        .collect()
}
pub fn interpret(request: &EnrichRequest) -> Interpretation {
    let original = request.description.clone();
    let (bank_text, mut unverified_tokens) = bank_formats::merchant_text(&original);
    let mut text = bank_text.trim();
    let mut processor_hint = None;
    while let Some((processor, rest)) = crate::regex_rules::processor_prefix(text) {
        // The outermost processor is the hint; nested wrappers are retained in original.
        processor_hint.get_or_insert_with(|| processor.to_owned());
        text = rest;
    }
    let mut words: Vec<_> = text.split_whitespace().collect();
    // Only remove terminal numeric tokens, preserving them as unverified clues.
    // Embedded numbers (7 Eleven, Studio 54) remain in the name hypotheses.
    while words.len() > 1
        && words
            .last()
            .is_some_and(|w| name_pattern("terminal-numbers").is_match(w))
    {
        unverified_tokens.insert(0, words.pop().unwrap().to_owned());
    }
    let full = words.join(" ");
    let mut hypotheses = vec![Hypothesis {
        merchant_text: full.clone(),
        possible_location: None,
        location_hint: None,
        geoname_ids: vec![],
    }];
    if processor_hint.as_deref() != Some("PayPal")
        && !crate::descriptor_location::billing_description(&original)
    {
        let mut extracted = crate::descriptor_location::extract(text, request);
        if extracted.is_empty() {
            extracted = crate::descriptor_location::extract(&full, request);
        }
        for extracted in extracted {
            hypotheses.push(Hypothesis {
                merchant_text: extracted.merchant_text,
                possible_location: extracted.possible_location,
                location_hint: Some(extracted.hint),
                geoname_ids: extracted.geoname_ids,
            });
        }
    }
    // Preserve the complete-name interpretation alongside possible trailing
    // locality spans; no gazetteer or model is allowed to make removal definitive.
    for trailing in 1..=2 {
        if words.len() > trailing {
            let at = words.len() - trailing;
            if !words[at..]
                .iter()
                .all(|w| name_pattern("trailing-localities").is_match(w))
            {
                continue;
            }
            hypotheses.push(Hypothesis {
                merchant_text: words[..at].join(" "),
                possible_location: Some(words[at..].join(" ")),
                location_hint: None,
                geoname_ids: vec![],
            });
        }
    }
    // Some acquirers send Merchant / City / Country. This is a possible
    // locality, never a verified purchase location or a country restriction.
    let parts: Vec<_> = full.split(" / ").collect();
    if parts.len() == 3 && name_pattern("slash-localities").is_match(parts[2]) {
        hypotheses.push(Hypothesis {
            merchant_text: parts[0].trim().to_owned(),
            possible_location: Some(parts[1..].join(" / ")),
            location_hint: None,
            geoname_ids: vec![],
        });
    }
    if normalize(&full) != normalize(&original) {
        hypotheses.push(Hypothesis {
            merchant_text: original.clone(),
            possible_location: None,
            location_hint: None,
            geoname_ids: vec![],
        });
    }
    let mut seen = std::collections::HashSet::new();
    hypotheses.retain(|h| {
        let name = normalize(&h.merchant_text);
        !name.is_empty() && seen.insert((name, h.possible_location.clone(), h.geoname_ids.clone()))
    });
    Interpretation {
        original,
        processor_hint,
        unverified_tokens,
        hypotheses,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn global_gazetteer_hypotheses_preserve_ambiguous_ids_and_region_context() {
        for description in ["CAFE / Springfield / USA", "CAFE SPRINGFIELD USA"] {
            let request =
                serde_json::from_value(serde_json::json!({"description":description})).unwrap();
            let parsed = interpret(&request);
            let hypothesis = parsed
                .hypotheses
                .iter()
                .find(|h| h.geoname_ids.len() > 1)
                .unwrap();
            assert_eq!(hypothesis.merchant_text, "CAFE");
            assert_eq!(
                hypothesis
                    .location_hint
                    .as_ref()
                    .unwrap()
                    .country
                    .as_deref(),
                Some("US")
            );
            assert!(hypothesis.location_hint.as_ref().unwrap().region.is_none());
            assert_eq!(parsed.original, description);
        }
        let request =
            serde_json::from_value(serde_json::json!({"description":"CAFE SPRINGFIELD IL USA"}))
                .unwrap();
        let parsed = interpret(&request);
        let hint = parsed
            .hypotheses
            .iter()
            .find_map(|h| h.location_hint.as_ref())
            .unwrap();
        assert_eq!(hint.region.as_deref(), Some("IL"));
        for description in [
            "CAFE / München / DEU",
            "CAFE TOKYO JPN",
            "CAFE LONDON GBR",
            "CAFE SYDNEY NEW SOUTH WALES AUS",
        ] {
            let request =
                serde_json::from_value(serde_json::json!({"description":description})).unwrap();
            assert!(
                interpret(&request)
                    .hypotheses
                    .iter()
                    .any(|h| h.merchant_text == "CAFE" && !h.geoname_ids.is_empty()),
                "{description}"
            );
        }
    }
    #[test]
    fn structured_locations_validate_whole_suffix_and_preserve_names() {
        for (description, city, region, country, store) in [
            (
                "SQ *CAFE STORE 00482 TORONTO ON CAN",
                "TORONTO",
                Some("ON"),
                "CA",
                Some("00482"),
            ),
            (
                "CAFE TORONTO ON #00482",
                "TORONTO",
                Some("ON"),
                "CA",
                Some("00482"),
            ),
            ("CAFE TORONTO ON 123456", "TORONTO", Some("ON"), "CA", None),
            (
                "CAFE SAN FRANCISCO CA",
                "SAN FRANCISCO",
                Some("CA"),
                "US",
                None,
            ),
            ("CAFE NEW YORK NY USA", "NEW YORK", Some("NY"), "US", None),
            ("CAFE MONTRÉAL QC CA", "MONTRÉAL", Some("QC"), "CA", None),
            (
                "CAFE BONDI BEACH NSW AUS",
                "BONDI BEACH",
                Some("NSW"),
                "AU",
                None,
            ),
            ("CAFE / Amsterdam / NLD", "Amsterdam", None, "NL", None),
            ("CAFE/Amsterdam/NL", "Amsterdam", None, "NL", None),
            ("CAFE AMSTERDAM NLD", "AMSTERDAM", None, "NL", None),
        ] {
            let request =
                serde_json::from_value(serde_json::json!({"description":description})).unwrap();
            let parsed = interpret(&request);
            assert_eq!(parsed.original, description);
            let hypothesis = parsed
                .hypotheses
                .iter()
                .find(|h| h.location_hint.is_some())
                .unwrap_or_else(|| panic!("{description}"));
            assert_eq!(hypothesis.merchant_text, "CAFE", "{description}");
            let hint = hypothesis.location_hint.as_ref().unwrap();
            assert_eq!(hint.city.as_deref(), Some(city), "{description}");
            assert_eq!(hint.region.as_deref(), region, "{description}");
            assert_eq!(hint.country.as_deref(), Some(country), "{description}");
            assert_eq!(hint.store_number.as_deref(), store, "{description}");
            assert!(hint.address.is_none() && hint.postal_code.is_none());
            assert!(
                parsed
                    .hypotheses
                    .iter()
                    .any(|h| h.merchant_text == description && h.location_hint.is_none())
            );
        }
        let request =
            serde_json::from_value(serde_json::json!({"description":"CAFE STORE 0006"})).unwrap();
        let parsed = interpret(&request);
        let hint = parsed
            .hypotheses
            .iter()
            .find_map(|h| h.location_hint.as_ref())
            .unwrap();
        assert_eq!(hint.store_number.as_deref(), Some("0006"));
        assert!(hint.city.is_none() && hint.region.is_none() && hint.country.is_none());
    }

    #[test]
    fn locations_reject_conflicts_billing_and_ambiguous_tokens() {
        for description in [
            "CAFE TORONTO NY",
            "CAFE TORONTO ON USA",
            "CAFE UNKNOWNCITY ON CAN",
            "CAFE / Unknowncity / CAN",
            "CAFE BROMONT",
            "CAFE CA",
            "CAFE 123456",
            "CAFE CARD #1234",
            "CAFE AUTH #1234",
            "CAFE #123 #456",
            "PAYPAL *CAFE TORONTO ON",
            "PP*CAFE TORONTO ON",
            "OPENAI SAN FRANCISCO CA",
            "ONLINE CAFE TORONTO ON",
            "GOOGLE *CAFE TORONTO ON",
        ] {
            let request =
                serde_json::from_value(serde_json::json!({"description":description})).unwrap();
            assert!(
                interpret(&request)
                    .hypotheses
                    .iter()
                    .all(|h| h.location_hint.is_none()),
                "{description}"
            );
        }
        for context in [
            serde_json::json!({"country":"US"}),
            serde_json::json!({"location":{"country":"US"}}),
            serde_json::json!({"location":{"city":"Montreal", "country":"CA"}}),
            serde_json::json!({"location":{"region":"QC", "country":"CA"}}),
        ] {
            let mut value = context;
            value["description"] = serde_json::json!("CAFE TORONTO ON CAN");
            let request = serde_json::from_value(value).unwrap();
            assert!(
                interpret(&request)
                    .hypotheses
                    .iter()
                    .all(|h| h.location_hint.is_none())
            );
        }
    }

    #[test]
    fn catalog_location_support_requires_compatible_and_unique_outlets() {
        let request = serde_json::from_value(serde_json::json!({
            "description":"JULIUS CAFE STORE 00482 TORONTO ON CAN"
        }))
        .unwrap();
        let parsed = interpret(&request);
        let merchant = serde_json::from_value(serde_json::json!({
            "id":"cafe", "name":"Julius Cafe", "markets":["CA"]
        }))
        .unwrap();
        let record: crate::location::LocationRecord = serde_json::from_value(serde_json::json!({
            "source":"reviewed-outlets", "external_id":"toronto-482", "aliases":["Julius Cafe Toronto"],
            "merchant":{"merchant_id":"cafe"},
            "location":{"id":null,"precision":"outlet", "city":"Toronto", "region":"ON",
                "country":"CA", "store_number":"00482"},
            "attribution":"Test fixture", "license":"test", "url":"https://example.com/outlet"
        })).unwrap();
        let evidence = catalog_support(&parsed, &merchant, std::slice::from_ref(&record), None);
        assert_eq!(
            evidence
                .iter()
                .find_map(|e| e.outlet.as_ref())
                .unwrap()
                .external_id,
            "toronto-482"
        );
        for (region, country, store) in [
            ("QC", "CA", "00482"),
            ("ON", "US", "00482"),
            ("ON", "CA", "00483"),
        ] {
            let mut conflicting = record.clone();
            conflicting.location.region = Some(region.into());
            conflicting.location.country = Some(country.into());
            conflicting.location.store_number = Some(store.into());
            assert!(
                catalog_support(&parsed, &merchant, &[conflicting], None)
                    .iter()
                    .all(|e| e.outlet.is_none())
            );
        }
        assert!(
            catalog_support(
                &parsed,
                &merchant,
                std::slice::from_ref(&record),
                Some("US")
            )
            .iter()
            .all(|e| e.outlet.is_none())
        );
        let mut other = record.clone();
        other.external_id = "other-outlet".into();
        assert!(
            catalog_support(&parsed, &merchant, &[record, other], None)
                .iter()
                .all(|e| e.outlet.is_none())
        );
    }

    #[test]
    fn ambiguous_localities_and_numbers_are_preserved_as_hypotheses() {
        let request = serde_json::from_value(
            serde_json::json!({"description":"SQ *JULIUS CAFE BROMONT 00482"}),
        )
        .unwrap();
        let parsed = interpret(&request);
        assert_eq!(parsed.processor_hint.as_deref(), Some("Square"));
        assert_eq!(parsed.unverified_tokens, ["00482"]);
        assert_eq!(parsed.hypotheses[0].merchant_text, "JULIUS CAFE BROMONT");
        assert_eq!(parsed.hypotheses[1].merchant_text, "JULIUS CAFE");
        assert_eq!(
            parsed.hypotheses[1].possible_location.as_deref(),
            Some("BROMONT")
        );
        for description in ["7 ELEVEN", "STUDIO 54", "東京 カフェ", "LS"] {
            let request =
                serde_json::from_value(serde_json::json!({"description":description})).unwrap();
            assert_eq!(interpret(&request).hypotheses[0].merchant_text, description);
        }
    }

    #[test]
    fn bank_formats_preserve_original_and_extract_merchant_hypotheses() {
        for (description, merchant, processor) in [
            ("CHECKCARD 1001 CARVANA REF: AB123", "CARVANA", None),
            ("31/12/2026 CARVANA", "CARVANA", None),
            (
                "CARVANA DES :PAYMENT INDN :RECIPIENT CO ID :123",
                "CARVANA",
                None,
            ),
            (
                "10/01 POS PURCHASE SQ *JULIUS CAFE 2026-10-01 REF: AB1234",
                "JULIUS CAFE",
                Some("Square"),
            ),
            ("PAYPAL *CARVANA 402-935-7733 AZ", "CARVANA", Some("PayPal")),
            ("PP* CARVANA (402) 935-7733", "CARVANA", Some("PayPal")),
            ("TST * JULIUS CAFE", "JULIUS CAFE", Some("Toast")),
            ("SP * STUDIO 54 ORDER #A1234", "STUDIO 54", Some("Shopify")),
            (
                "PADDLE.NET* SOFTWARE REF:ABC123",
                "SOFTWARE",
                Some("Paddle"),
            ),
            ("gosq.com JULIUS CAFE", "JULIUS CAFE", Some("Square")),
            ("SQ * PAYPAL * JULIUS CAFE", "JULIUS CAFE", Some("Square")),
            (
                "Orig CO Name:JULIUS CAFE Orig ID:123456 Desc Date:261001 CO Entry Descr:PAYMENT Sec:CCD Trace#:123456789 Eed:261002 Ind Name:RECIPIENT",
                "JULIUS CAFE",
                None,
            ),
            (
                "JULIUS CAFE DES:PAYMENT ID:123456 INDN:RECIPIENT CO ID:987654 PPD",
                "JULIUS CAFE",
                None,
            ),
            (
                "Adyen Tech Support / Amsterdam / NLD",
                "Adyen Tech Support",
                None,
            ),
            ("東京 カフェ 2026/10/01 REF: X123", "東京 カフェ", None),
        ] {
            let request =
                serde_json::from_value(serde_json::json!({"description":description})).unwrap();
            let parsed = interpret(&request);
            assert_eq!(parsed.original, description);
            assert_eq!(parsed.processor_hint.as_deref(), processor, "{description}");
            assert!(
                parsed
                    .hypotheses
                    .iter()
                    .any(|h| h.merchant_text == merchant),
                "{description}: {:?}",
                parsed.hypotheses
            );
            assert!(
                parsed
                    .hypotheses
                    .iter()
                    .any(|h| h.merchant_text == description),
                "{description}"
            );
            assert!(
                !parsed
                    .hypotheses
                    .iter()
                    .any(|h| h.merchant_text == "RECIPIENT")
            );
        }
    }

    #[test]
    fn ambiguous_names_dates_references_and_processor_boundaries_survive() {
        for description in [
            "7 ELEVEN",
            "STUDIO 54",
            "FOREVER 21",
            "SQUID CAFE",
            "SPAR",
            "LS",
            "PAYPAL",
            "SQ*",
            "ACME DES: STUDIO",
            "ACME REF: BOOKS",
            "ACME A10/01B",
            "CAFE 99/99",
            "CAFE 13/99/2026",
            "CAFE 2026-99-10",
        ] {
            let request =
                serde_json::from_value(serde_json::json!({"description":description})).unwrap();
            let parsed = interpret(&request);
            assert_eq!(parsed.hypotheses[0].merchant_text, description);
            assert!(parsed.processor_hint.is_none(), "{description}");
        }
        let description = "POS PURCHASE 10/01 REF: AB123";
        let request =
            serde_json::from_value(serde_json::json!({"description":description})).unwrap();
        let parsed = interpret(&request);
        assert_eq!(parsed.hypotheses.len(), 1);
        assert_eq!(parsed.hypotheses[0].merchant_text, description);
        assert!(parsed.unverified_tokens.iter().any(|s| s == "REF: AB123"));
    }
}
