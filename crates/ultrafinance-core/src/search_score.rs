use rapidfuzz::distance::levenshtein::BatchComparator;
use std::collections::{HashMap, HashSet};

/// Reuse query tokenization and edit distances within one search.
pub(crate) struct Scorer<'a> {
    query: &'a str,
    query_comparator: BatchComparator<char>,
    query_len: usize,
    tokens: Vec<&'a str>,
    token_comparators: Vec<BatchComparator<char>>,
    scores: HashMap<String, f64>,
    token_scores: HashMap<String, Vec<f64>>,
    best: Vec<f64>,
}
impl<'a> Scorer<'a> {
    pub(crate) fn new(query: &'a str) -> Self {
        let mut tokens: Vec<_> = query
            .split_whitespace()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        tokens.sort_unstable();
        Self {
            query,
            query_comparator: BatchComparator::new(query.chars()),
            query_len: query.chars().count(),
            token_comparators: tokens
                .iter()
                .map(|t| BatchComparator::new(t.chars()))
                .collect(),
            best: vec![0.0; tokens.len()],
            tokens,
            scores: HashMap::new(),
            token_scores: HashMap::new(),
        }
    }
    pub(crate) fn score(&mut self, name: String) -> f64 {
        if let Some(score) = self.scores.get(&name) {
            return *score;
        }
        let mut words: Vec<_> = name.split_whitespace().collect();
        words.sort_unstable();
        words.dedup();
        let intersection = self
            .tokens
            .iter()
            .filter(|token| words.binary_search(token).is_ok())
            .count();
        let union = self.tokens.len() + words.len() - intersection;
        let overlap = intersection as f64 / union.max(1) as f64;
        self.best.fill(0.0);
        for word in words {
            let distances = self
                .token_scores
                .entry(word.to_string())
                .or_insert_with(|| {
                    self.token_comparators
                        .iter()
                        .map(|comparator| comparator.normalized_similarity(word.chars()))
                        .collect()
                });
            for (best, score) in self.best.iter_mut().zip(distances) {
                *best = best.max(*score);
            }
        }
        let token_similarity = self.best.iter().sum::<f64>() / self.tokens.len().max(1) as f64;
        // At least the length difference must be edited. If even that best possible
        // whole-string score cannot beat token similarity, its exact distance is irrelevant.
        let name_len = name.chars().count();
        let upper = 1.0
            - self.query_len.abs_diff(name_len) as f64 / self.query_len.max(name_len).max(1) as f64;
        let whole = if upper <= token_similarity {
            token_similarity
        } else {
            self.query_comparator
                .normalized_similarity(name.chars())
                .max(token_similarity)
        };
        let score = (0.7 * whole + 0.3 * overlap).max(contained_name_score(self.query, &name));
        self.scores.insert(name, score);
        score
    }
}

// A complete name inside a noisy description is strong retrieval evidence, but
// never an exact/trusted match. Respect token boundaries and avoid boosting
// short abbreviations or names consisting entirely of transaction vocabulary.
fn contained_name_score(query: &str, name: &str) -> f64 {
    let meaningful = name.split_whitespace().any(|word| {
        word.chars().count() >= 3
            && !matches!(
                word,
                "purchase"
                    | "payment"
                    | "transfer"
                    | "credit"
                    | "debit"
                    | "refund"
                    | "store"
                    | "online"
                    | "transaction"
                    | "pending"
                    | "card"
                    | "pos"
                    | "visa"
                    | "mastercard"
                    | "from"
                    | "to"
            )
            && word.chars().any(char::is_alphabetic)
    });
    if !meaningful || name.chars().count() < 5 {
        return 0.0;
    }
    let contained = query.match_indices(name).any(|(start, _)| {
        (start == 0 || query.as_bytes()[start - 1] == b' ')
            && (start + name.len() == query.len() || query.as_bytes()[start + name.len()] == b' ')
    });
    if contained {
        // Prefer the more specific name when both a brand and a longer business
        // name occur in the same description. Keep exact full-query scores higher.
        0.85 + 0.1 * name.chars().count() as f64 / query.chars().count().max(1) as f64
    } else {
        0.0
    }
}
#[cfg(test)]
#[path = "../tests/unit/search_score/tests.rs"]
mod tests;
