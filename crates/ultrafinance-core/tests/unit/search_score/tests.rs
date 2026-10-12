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
    (0.7 * strsim::normalized_levenshtein(query, name).max(token) + 0.3 * overlap)
        .max(contained_name_score(query, name))
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
fn cached_scoring_preserves_unicode_and_scoring_formula() {
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
            let expected = (0.7 * strsim::normalized_levenshtein(query, name).max(token)
                + 0.3 * overlap)
                .max(contained_name_score(query, name));
            assert!(
                (scorer.score(name.into()) - expected).abs() < 1e-12,
                "{query} / {name}"
            );
            assert!((scorer.score(name.into()) - expected).abs() < 1e-12);
        }
    }
}

#[test]
fn embedded_names_respect_boundaries_specificity_and_noise() {
    let query = "credit card purchase amazon web services store 1005 montreal qc 2026 10 01";
    let mut scorer = Scorer::new(query);
    assert!(scorer.score("amazon web services".into()) > 0.85);
    assert!(scorer.score("amazon web services".into()) > scorer.score("amazon".into()));
    assert_eq!(contained_name_score(query, "credit card"), 0.0);
    assert_eq!(contained_name_score(query, "1005"), 0.0);
    assert_eq!(
        contained_name_score("payment carpet supertarget 123", "target"),
        0.0
    );
    assert_eq!(contained_name_score("payment uber 123", "uber"), 0.0);
    assert_eq!(
        contained_name_score("payment alpha cafe 123", "cafe alpha"),
        0.0
    );
    assert!(Scorer::new("amazon").score("amazon".into()) > scorer.score("amazon".into()));
}
