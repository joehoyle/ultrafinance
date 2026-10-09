//! Descriptor locality clues validated against the pinned offline GeoNames index.
use crate::{EnrichRequest, location::LocationHint, store::normalize};
use regex::Regex;
use std::sync::OnceLock;

pub(crate) fn billing_description(description: &str) -> bool {
    let text = normalize(description).to_ascii_uppercase();
    let billing = [
        "GOOGLE",
        "PAYPAL",
        "SUBSCRIPTION",
        "SUBSCR",
        "ONLINE",
        "OPENAI",
        "ANTHROPIC",
        "CLAUDE",
        "HULU",
        "NETFLIX",
        "DISNEY PLUS",
        "QANTAS",
    ];
    let padded = format!(" {text} ");
    billing.iter().any(|s| padded.contains(&format!(" {s} "))) || text.contains("APPLE COM BILL")
}

pub(crate) fn store_number(description: &str) -> Option<String> {
    // Do not extract an arbitrary numeric token: dates, cards, and phones abound.
    let store = store_pattern();
    let identifiers: std::collections::BTreeSet<_> = store
        .captures_iter(description)
        .filter_map(|captures| {
            let found = captures.get(0)?;
            let prefix = &description[..found.start()];
            // A hash embedded in a card/reference token is not a store marker.
            if found.as_str().starts_with('#')
                && prefix.chars().last().is_some_and(|c| !c.is_whitespace())
            {
                return None;
            }
            let preceding = prefix
                .split_whitespace()
                .last()
                .unwrap_or("")
                .to_ascii_uppercase();
            if [
                "CARD", "ACCOUNT", "AUTH", "ORDER", "REF", "RECEIPT", "PHONE",
            ]
            .contains(&preceding.as_str())
            {
                return None;
            }
            Some(captures[1].to_ascii_uppercase())
        })
        .collect();
    if identifiers.len() == 1 {
        identifiers.into_iter().next()
    } else {
        None
    }
}

const SUFFIX: crate::interpretation::Format = crate::interpretation::Format {
    id: "validated-location-suffixes",
    description: "Parse city + region + optional country, or city + country, from the right; validate the whole combination against the offline gazetteer and caller context.",
    pattern: Some(r"(?i)\b[\p{L}.-]+$"),
};
const SLASH: crate::interpretation::Format = crate::interpretation::Format {
    id: "validated-slash-locations",
    description: "Parse Merchant / City / Country with a gazetteer-supported city and country; retain the full name as a competing hypothesis.",
    pattern: Some(r"(?i)^.+?\s*/\s*.+?\s*/\s*[A-Z]{2,3}$"),
};
const STORE: crate::interpretation::Format = crate::interpretation::Format {
    id: "store-markers",
    description: "Extract a single explicit #ID or STORE ID, preserving leading zeros; reject card/reference markers and conflicting store IDs. Remove a terminal store marker only from a competing name hypothesis.",
    pattern: Some(r"(?i)(?:#|\bSTORE\s+)([A-Z0-9]{1,16})\b"),
};

pub(crate) fn formats() -> impl Iterator<Item = crate::interpretation::Format> {
    [SUFFIX, SLASH, STORE].into_iter()
}

fn store_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(STORE.pattern.unwrap()).unwrap())
}

pub(crate) struct Extracted {
    pub merchant_text: String,
    pub possible_location: Option<String>,
    pub hint: LocationHint,
    pub geoname_ids: Vec<u32>,
    pub canonical_city: Option<&'static str>,
}

fn strip_store_suffix(merchant: &str, store: Option<&str>) -> String {
    if let Some(store) = store {
        for captures in store_pattern().captures_iter(merchant) {
            let span = captures.get(0).unwrap();
            if span.end() == merchant.len()
                && captures[1].eq_ignore_ascii_case(store)
                && store_number(merchant).as_deref() == Some(store)
            {
                return merchant[..span.start()].trim().to_owned();
            }
        }
    }
    merchant.to_owned()
}

/// Parsed hints do not establish purchase geography. Multiple places sharing a
/// city/country name retain all matching IDs rather than choosing by population.
pub(crate) fn extract(text: &str, request: &EnrichRequest) -> Vec<Extracted> {
    let store = store_number(text);
    let original_text = text;
    let without_store = strip_store_suffix(text, store.as_deref());
    let text = without_store.as_str();
    let words: Vec<_> = text.split_whitespace().collect();
    let parts: Vec<_> = text.split('/').map(str::trim).collect();
    let (max_city_words, max_region_words) = crate::gazetteer::limits();
    static SUFFIX_PATTERN: OnceLock<Regex> = OnceLock::new();
    static SLASH_PATTERN: OnceLock<Regex> = OnceLock::new();
    let suffix = SUFFIX_PATTERN.get_or_init(|| Regex::new(SUFFIX.pattern.unwrap()).unwrap());
    let slash = SLASH_PATTERN.get_or_init(|| Regex::new(SLASH.pattern.unwrap()).unwrap());
    let mut found = Vec::new();
    if parts.len() == 3 && slash.is_match(text) {
        for city in crate::gazetteer::lookup(parts[1], Some(parts[2]), None) {
            found.push((
                words.len(),
                parts[0].to_owned(),
                parts[1..].join(" / "),
                parts[1].to_owned(),
                false,
                city,
            ));
        }
    } else if suffix.is_match(text.trim_end_matches(',')) {
        for has_country in [true, false] {
            let explicit_country = if has_country {
                match words
                    .last()
                    .and_then(|code| crate::gazetteer::country(&normalize(code)))
                {
                    Some(country) => Some(country),
                    None => continue,
                }
            } else {
                request.country.as_deref().or_else(|| {
                    request
                        .location
                        .as_ref()
                        .and_then(|hint| hint.country.as_deref())
                })
            };
            let end = words.len() - usize::from(has_country);
            for region_words in
                usize::from(!has_country)..=max_region_words.min(end.saturating_sub(2))
            {
                let city_end = end - region_words;
                let region = (region_words > 0).then(|| words[city_end..end].join(" "));
                for city_words in 1..=max_city_words.min(city_end.saturating_sub(1)) {
                    let at = city_end - city_words;
                    let raw_city = words[at..city_end].join(" ");
                    for city in
                        crate::gazetteer::lookup(&raw_city, explicit_country, region.as_deref())
                    {
                        found.push((
                            words.len() - at,
                            words[..at].join(" "),
                            words[at..].join(" "),
                            raw_city.clone(),
                            region_words > 0,
                            city,
                        ));
                    }
                }
            }
        }
    }
    let mut groups: Vec<(usize, Extracted)> = Vec::new();
    for (length, merchant, raw_location, raw_city, has_region, city) in found {
        if request
            .country
            .as_deref()
            .is_some_and(|country| country != city.country)
        {
            continue;
        }
        let hint = LocationHint {
            city: Some(raw_city),
            region: has_region.then(|| city.region.into()),
            country: Some(city.country.into()),
            store_number: store.clone(),
            ..Default::default()
        };
        if let Some(context) = &request.location {
            if context.city.as_ref().is_some_and(|name| {
                !crate::gazetteer::lookup(name, Some(city.country), None)
                    .iter()
                    .any(|known| known.geoname_id == city.geoname_id)
            }) || context
                .region
                .as_ref()
                .is_some_and(|region| !crate::gazetteer::region_matches(city, region))
            {
                continue;
            }
            let mut context = context.clone();
            context.city = None;
            context.region = None;
            if !crate::location::compatible(&context, &hint) {
                continue;
            }
        }
        let merchant = strip_store_suffix(&merchant, store.as_deref());
        if normalize(&merchant).is_empty() {
            continue;
        }
        if let Some((_, group)) = groups.iter_mut().find(|(_, group)| {
            group.merchant_text == merchant
                && group.possible_location.as_deref() == Some(&raw_location)
                && group.hint == hint
        }) {
            group.geoname_ids.push(city.geoname_id);
        } else {
            groups.push((
                length,
                Extracted {
                    merchant_text: merchant,
                    possible_location: Some(raw_location),
                    hint,
                    geoname_ids: vec![city.geoname_id],
                    canonical_city: Some(city.city),
                },
            ));
        }
    }
    if let Some(longest) = groups.iter().map(|(length, _)| *length).max() {
        groups.retain(|(length, _)| *length == longest);
    }
    // Bound provider work without selecting an arbitrary subset of alternatives.
    if groups.len() > 8 {
        return vec![];
    }
    let mut result: Vec<_> = groups
        .into_iter()
        .map(|(_, mut group)| {
            group.geoname_ids.sort_unstable();
            group.geoname_ids.dedup();
            if group.geoname_ids.len() != 1 {
                group.canonical_city = None;
            }
            group
        })
        .collect();
    result.sort_by(|a, b| a.geoname_ids.cmp(&b.geoname_ids));
    if result.is_empty() && text != original_text && !normalize(text).is_empty() {
        let hint = LocationHint {
            store_number: store,
            ..Default::default()
        };
        if request
            .location
            .as_ref()
            .is_none_or(|context| crate::location::compatible(context, &hint))
        {
            result.push(Extracted {
                merchant_text: text.into(),
                possible_location: None,
                hint,
                geoname_ids: vec![],
                canonical_city: None,
            });
        }
    }
    result
}
