//! Retrieval diagnostics and the final, name-diverse request shortlist.
use super::{Candidate, normalize};
use crate::interpretation::Interpretation;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize)]
pub struct RetrievalEvidence {
    pub path: &'static str,
    pub query: String,
    pub score: f64,
}

#[derive(Debug, Default, Serialize)]
pub struct SearchTrace {
    pub ranking_country: Option<String>,
    pub query_pool_limit: usize,
    pub name_limit: usize,
    pub considered: Vec<RankedCandidate>,
}

#[derive(Debug, Serialize)]
pub struct RankedCandidate {
    pub merchant_id: String,
    pub name: String,
    pub score: f64,
    pub country_match: bool,
    pub location_only: bool,
    pub shadowed_by: Option<String>,
    pub name_prefix: bool,
    pub selected: bool,
    pub retrieval: Vec<RetrievalEvidence>,
}

pub(super) fn alias_spans(query: &str) -> Vec<String> {
    // At most 81 spans from the first 16 tokens. Keep short tokens inside
    // phrases (Amazon.ca, H&M); standalone short codes stay exact-only.
    let words: Vec<_> = query.split_whitespace().take(16).collect();
    let mut spans = Vec::new();
    let mut seen = HashSet::new();
    for width in (1..=6.min(words.len())).rev() {
        for part in words.windows(width) {
            let span = part.join(" ");
            if span != query
                && span.chars().filter(|c| c.is_alphanumeric()).count() >= 3
                && part
                    .iter()
                    .any(|word| word.chars().any(char::is_alphabetic))
                && seen.insert(span.clone())
            {
                spans.push(span);
            }
        }
    }
    spans
}

fn location_only(candidate: &Candidate, interpretation: &Interpretation) -> bool {
    // Only structured location interpretations can demote a match. Keep full
    // names as alternatives and never demote a raw exact alias or source rule.
    if candidate.exact
        || candidate.regex_match_length.is_some()
        || candidate.resolution_id.is_some()
    {
        return false;
    }
    let names: Vec<_> = std::iter::once(&candidate.merchant.name)
        .chain(&candidate.merchant.aliases)
        .map(|s| normalize(s))
        .collect();
    let structured: Vec<_> = interpretation
        .hypotheses
        .iter()
        .filter(|h| h.location_hint.is_some() && h.possible_location.is_some())
        .collect();
    !structured.is_empty()
        && structured.iter().all(|h| {
            let locality = normalize(h.possible_location.as_deref().unwrap());
            let merchant = normalize(&h.merchant_text);
            names.iter().any(|name| {
                name.split_whitespace().any(|word| {
                    word.chars().count() >= 3 && locality.split_whitespace().any(|w| w == word)
                })
            }) && !names.iter().any(|name| {
                name.split_whitespace().any(|word| {
                    word.chars().count() >= 3 && merchant.split_whitespace().any(|w| w == word)
                })
            })
        })
}

pub(super) fn shortlist(
    mut found: Vec<Candidate>,
    interpretation: &Interpretation,
    limit: usize,
    country: Option<&str>,
    pool_limit: usize,
) -> (Vec<Candidate>, SearchTrace) {
    let mut locality_ids = HashSet::new();
    let mut shadowed = HashMap::new();
    let mut prefix_ids = HashSet::new();
    // Exact alias spans are lexical evidence. Prefer a complete observed name
    // to its substrings, without merging those businesses or dropping variants.
    let observed: HashMap<_, Vec<_>> = found
        .iter()
        .map(|candidate| {
            let spans = candidate
                .retrieval_evidence
                .iter()
                .filter(|e| matches!(e.path, "alias_span" | "exact_alias"))
                .map(|e| e.query.clone())
                .collect();
            (candidate.merchant.id.clone(), spans)
        })
        .collect();
    let mut all_spans: Vec<_> = observed.values().flatten().cloned().collect();
    all_spans.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    all_spans.dedup();
    let name_texts: Vec<_> = interpretation
        .hypotheses
        .iter()
        .map(|h| normalize(&h.merchant_text))
        .collect();
    for candidate in &mut found {
        let spans = &observed[&candidate.merchant.id];
        let protected = candidate.exact
            || candidate.regex_match_length.is_some()
            || candidate.resolution_id.is_some();
        let best_span = spans.iter().max_by_key(|s| s.len());
        let longer = best_span.and_then(|span| {
            all_spans.iter().find(|other| {
                other.len() > span.len() && format!(" {other} ").contains(&format!(" {span} "))
            })
        });
        if !protected {
            if let Some(longer) = longer {
                shadowed.insert(candidate.merchant.id.clone(), longer.clone());
                candidate.score = candidate.score.min(0.34);
            } else if spans.iter().any(|span| {
                name_texts
                    .iter()
                    .any(|text| text == span || text.starts_with(&format!("{span} ")))
            }) {
                // Descriptors and parsed merchant fields normally put the name
                // first. This is a small retrieval preference, never match trust.
                prefix_ids.insert(candidate.merchant.id.clone());
                candidate.score = (candidate.score + 0.03).min(0.97);
            }
        }
        if location_only(candidate, interpretation) {
            locality_ids.insert(candidate.merchant.id.clone());
            candidate.score = candidate.score.min(0.30);
        }
    }
    // Sort the entire retrieved pool before spending the name budget. The
    // existing sorter caps results at 254; share its comparator and retain all
    // considered rows in the trace.
    found.sort_by(|a, b| super::candidate_order(a, b, country));
    let mut names = HashSet::new();
    let mut selected_ids = HashSet::new();
    for candidate in &found {
        if names.len() < limit || candidate.exact || candidate.regex_match_length.is_some() {
            names.insert(normalize(&candidate.merchant.name));
        }
    }
    let mut groups: HashMap<String, Vec<Candidate>> = HashMap::new();
    for candidate in &found {
        let name = normalize(&candidate.merchant.name);
        if names.contains(&name) {
            groups.entry(name).or_default().push(candidate.clone());
        }
    }
    let mut candidates = Vec::new();
    // Keep groups whole at the provider's record ceiling. Truncating a group to
    // one record would manufacture a resolved identity from an ambiguous name.
    for candidate in &found {
        if let Some(group) = groups.remove(&normalize(&candidate.merchant.name))
            && candidates.len() + group.len() <= 254
        {
            candidates.extend(group);
        }
    }
    candidates.sort_by(|a, b| super::candidate_order(a, b, country));
    selected_ids.extend(candidates.iter().map(|c| c.merchant.id.clone()));
    let trace = SearchTrace {
        ranking_country: country.map(str::to_owned),
        query_pool_limit: pool_limit,
        name_limit: limit,
        considered: found
            .into_iter()
            .map(|c| RankedCandidate {
                selected: selected_ids.contains(&c.merchant.id),
                location_only: locality_ids.contains(&c.merchant.id),
                shadowed_by: shadowed.remove(&c.merchant.id),
                name_prefix: prefix_ids.contains(&c.merchant.id),
                country_match: country
                    .is_some_and(|country| c.merchant.markets.iter().any(|m| m == country)),
                merchant_id: c.merchant.id,
                name: c.merchant.name,
                score: c.score,
                retrieval: c.retrieval_evidence,
            })
            .collect(),
    };
    (candidates, trace)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: usize, name: &str, score: f64) -> Candidate {
        Candidate {
            merchant: serde_json::from_value(serde_json::json!({
                "id":format!("id-{id:04}"),"name":name,"markets":[]
            }))
            .unwrap(),
            score,
            exact: false,
            trusted: false,
            regex_match_length: None,
            provenance: vec![],
            interpretation_evidence: vec![],
            business_evidence: serde_json::Value::Null,
            retrieval_evidence: vec![],
            resolution_id: None,
            pending_import: false,
        }
    }

    #[test]
    fn span_lookup_is_bounded_and_retains_domain_suffixes() {
        let spans = alias_spans("amazon ca 353873pq3 toronto on");
        assert!(spans.contains(&"amazon ca".into()));
        assert!(!spans.contains(&"ca".into()));
        assert!(!spans.contains(&"on".into()));
        assert!(alias_spans("ls").is_empty());
        let long = (0..100)
            .map(|i| format!("word{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(alias_spans(&long).len(), 81);
    }

    #[test]
    fn provider_record_ceiling_never_splits_an_identity_group() {
        let request =
            serde_json::from_value(serde_json::json!({"description":"Alpha Beta"})).unwrap();
        let interpretation = crate::interpretation::interpret(&request);
        let mut pool = vec![candidate(0, "Alpha", 0.99)];
        pool.extend((1..=254).map(|id| candidate(id, "Beta", 0.9)));
        let (hits, trace) = shortlist(pool, &interpretation, 2, None, 254);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].merchant.name, "Alpha");
        assert_eq!(trace.considered.iter().filter(|c| !c.selected).count(), 254);
    }

    #[test]
    fn locality_overlap_alone_does_not_compete_with_the_merchant_name() {
        let request =
            serde_json::from_value(serde_json::json!({"description":"YouTubePremium HALIFAX NS"}))
                .unwrap();
        let interpretation = crate::interpretation::interpret(&request);
        let (hits, trace) = shortlist(
            vec![
                candidate(0, "Best Halifax Plumber", 0.89),
                candidate(1, "YouTube", 0.7),
            ],
            &interpretation,
            1,
            None,
            64,
        );
        assert_eq!(hits[0].merchant.name, "YouTube");
        assert!(
            trace
                .considered
                .iter()
                .any(|c| c.name == "Best Halifax Plumber" && c.location_only)
        );
    }
}
