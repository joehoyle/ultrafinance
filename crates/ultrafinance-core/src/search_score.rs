use rapidfuzz::distance::levenshtein::BatchComparator;
use std::collections::{HashMap, HashSet};

/// Reuse query tokenization and edit distances within one search.
pub(crate) struct Scorer<'a> {
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
        let score = 0.7 * whole + 0.3 * overlap;
        self.scores.insert(name, score);
        score
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn reference(query: &str, name: &str) -> f64 {
        let aa: HashSet<_> = query.split_whitespace().collect();
        let bb: HashSet<_> = name.split_whitespace().collect();
        let overlap = aa.intersection(&bb).count() as f64 / aa.union(&bb).count().max(1) as f64;
        let token = aa
            .iter()
            .map(|a| {
                bb.iter()
                    .map(|b| strsim::normalized_levenshtein(a, b))
                    .fold(0.0, f64::max)
            })
            .sum::<f64>()
            / aa.len().max(1) as f64;
        0.7 * strsim::normalized_levenshtein(query, name).max(token) + 0.3 * overlap
    }
    #[test]
    fn optimized_distance_matches_reference_across_lengths_and_unicode() {
        let alphabet: Vec<_> = "abcde 123é東京ß".chars().collect();
        let mut state = 17_u64;
        let mut word = |length: usize| -> String {
            (0..length)
                .map(|_| {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    alphabet[(state >> 32) as usize % alphabet.len()]
                })
                .collect()
        };
        for length in [0, 1, 2, 7, 31, 63, 64, 65, 127, 128, 129, 256] {
            let query = word(length);
            let mut scorer = Scorer::new(&query);
            for name_length in [0, 1, 7, 63, 64, 65, 127, 129, 200] {
                let name = word(name_length);
                let expected = reference(&query, &name);
                assert!((scorer.score(name.clone()) - expected).abs() < 1e-12);
                assert!((scorer.score(name) - expected).abs() < 1e-12);
            }
        }
    }
    #[test]
    fn cached_scoring_preserves_unicode_and_original_scoring_formula() {
        for query in [
            "payment from adidas",
            "julius cafe bromont",
            "café 東京",
            "",
            "amazon web services 123456",
            "repeat repeat words",
            "credit card payment from adidas store 1005 montreal qc 2026 10 01",
        ] {
            let mut scorer = Scorer::new(query);
            for name in [
                "adidas",
                "julius café",
                "café 東京",
                "",
                "amazon web services",
                "words repeat",
            ] {
                let aa: HashSet<_> = query.split_whitespace().collect();
                let bb: HashSet<_> = name.split_whitespace().collect();
                let overlap =
                    aa.intersection(&bb).count() as f64 / aa.union(&bb).count().max(1) as f64;
                let token = aa
                    .iter()
                    .map(|a| {
                        bb.iter()
                            .map(|b| strsim::normalized_levenshtein(a, b))
                            .fold(0.0, f64::max)
                    })
                    .sum::<f64>()
                    / aa.len().max(1) as f64;
                let expected =
                    0.7 * strsim::normalized_levenshtein(query, name).max(token) + 0.3 * overlap;
                assert!(
                    (scorer.score(name.into()) - expected).abs() < 1e-12,
                    "{query} / {name}"
                );
                assert!((scorer.score(name.into()) - expected).abs() < 1e-12);
            }
        }
    }
}
