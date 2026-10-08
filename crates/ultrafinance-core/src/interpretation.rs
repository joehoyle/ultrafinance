//! Bounded hypotheses copied from the descriptor, never verified business facts.
use crate::{EnrichRequest, store::normalize};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Hypothesis {
    pub merchant_text: String,
    pub possible_location: Option<String>,
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
    pub matched_name: String,
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

/// Name equality and stored city equality support an interpretation, not trust.
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
            let name = std::iter::once(&merchant.name)
                .chain(&merchant.aliases)
                .find(|name| normalize(name) == normalize(&hypothesis.merchant_text))?;
            let outlet = hypothesis.possible_location.as_ref().and_then(|city| {
                outlets
                    .iter()
                    .find(|record| {
                        record
                            .location
                            .city
                            .as_ref()
                            .is_some_and(|known| normalize(known) == normalize(city))
                            && country.is_none_or(|country| {
                                record.location.country.as_deref() == Some(country)
                            })
                    })
                    .map(|record| OutletSupport {
                        source: record.source.clone(),
                        external_id: record.external_id.clone(),
                        city: record.location.city.clone().unwrap(),
                        country: record.location.country.clone(),
                        attribution: record.attribution.clone(),
                        license: record.license.clone(),
                        url: record.url.clone(),
                    })
            });
            Some(CatalogSupport {
                merchant_text: hypothesis.merchant_text.clone(),
                possible_location: hypothesis.possible_location.clone(),
                matched_name: name.clone(),
                outlet,
            })
        })
        .collect()
}
pub fn interpret(request: &EnrichRequest) -> Interpretation {
    let original = request.description.clone();
    let mut text = original.trim();
    let mut processor_hint = None;
    for (prefix, processor) in [
        ("SQ *", "Square"),
        ("SQ*", "Square"),
        ("TST*", "Toast"),
        ("TST *", "Toast"),
        ("PAYPAL *", "PayPal"),
        ("PAYPAL*", "PayPal"),
    ] {
        if text
            .get(..prefix.len())
            .is_some_and(|s| s.eq_ignore_ascii_case(prefix))
        {
            processor_hint = Some(processor.to_owned());
            text = text[prefix.len()..].trim();
            break;
        }
    }
    let mut words: Vec<_> = text.split_whitespace().collect();
    let mut unverified_tokens = Vec::new();
    // Only remove terminal numeric tokens, preserving them as unverified clues.
    // Embedded numbers (7 Eleven, Studio 54) remain in the name hypotheses.
    while words.len() > 1
        && words.last().is_some_and(|w| {
            w.trim_start_matches('#').chars().count() >= 3
                && w.trim_start_matches('#')
                    .chars()
                    .all(|c| c.is_ascii_digit())
        })
    {
        unverified_tokens.insert(0, words.pop().unwrap().to_owned());
    }
    let full = words.join(" ");
    let mut hypotheses = vec![Hypothesis {
        merchant_text: full.clone(),
        possible_location: None,
    }];
    // Preserve the complete-name interpretation alongside possible trailing
    // locality spans; no gazetteer or model is allowed to make removal definitive.
    for trailing in 1..=2 {
        if words.len() >= trailing + 2 {
            let at = words.len() - trailing;
            if !words[at..]
                .iter()
                .all(|w| w.chars().any(char::is_alphabetic))
            {
                continue;
            }
            hypotheses.push(Hypothesis {
                merchant_text: words[..at].join(" "),
                possible_location: Some(words[at..].join(" ")),
            });
        }
    }
    if normalize(&full) != normalize(&original) {
        hypotheses.push(Hypothesis {
            merchant_text: original.clone(),
            possible_location: None,
        });
    }
    hypotheses.retain(|h| !normalize(&h.merchant_text).is_empty());
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
}
