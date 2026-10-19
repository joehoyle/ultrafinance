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
    // A bank's machine-supplied merchant name can help retrieve candidates,
    // but remains unverified evidence alongside the original descriptor.
    if let Some(name) = request
        .extra
        .get("bank")
        .and_then(|bank| bank.get("merchant_name"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty() && name.len() <= 512)
    {
        hypotheses.push(Hypothesis {
            merchant_text: name.to_owned(),
            possible_location: None,
            location_hint: request.location.clone(),
            geoname_ids: vec![],
        });
    }
    // A store number may become terminal only after a locality is extracted.
    // Add a cleaned hypothesis while preserving every original interpretation.
    let cleaned: Vec<_> = hypotheses
        .iter()
        .filter_map(|h| {
            let mut words: Vec<_> = h.merchant_text.split_whitespace().collect();
            let before = words.len();
            while words.len() > 1
                && words.last().is_some_and(|w| {
                    name_pattern("terminal-numbers").is_match(w) || matches!(*w, "#" | "-")
                })
            {
                words.pop();
            }
            if before == words.len() {
                return None;
            }
            let mut next = h.clone();
            next.merchant_text = words.join(" ");
            Some(next)
        })
        .collect();
    hypotheses.extend(cleaned);
    // Delimited long references are alternative interpretations, never aliases.
    // Do not strip arbitrary numeric brand names or alphabetic product suffixes.
    let without_references: Vec<_> = hypotheses
        .iter()
        .filter_map(|hypothesis| {
            let text = &hypothesis.merchant_text;
            let (at, reference) = text.match_indices('*').find_map(|(at, _)| {
                let reference = text[at + 1..].split_whitespace().next()?;
                (at > 0
                    && reference.len() >= 6
                    && reference.chars().all(|c| c.is_ascii_alphanumeric())
                    && reference.chars().any(|c| c.is_ascii_digit()))
                .then_some((at, reference))
            })?;
            let removed = format!("*{reference}");
            if !unverified_tokens.contains(&removed) {
                unverified_tokens.push(removed);
            }
            let mut next = hypothesis.clone();
            next.merchant_text = format!("{} {}", &text[..at], &text[at + 1 + reference.len()..])
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            Some(next)
        })
        .collect();
    hypotheses.extend(without_references);
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
#[path = "../tests/unit/interpretation/tests.rs"]
mod tests;
