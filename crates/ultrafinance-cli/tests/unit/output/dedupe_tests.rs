use super::*;
use ultrafinance_core::dedupe::{Decision, Report};

#[test]
fn candidate_pairs_are_adjacent_even_when_catalog_ids_interleave() {
    let merchants = serde_json::from_value(serde_json::json!([
        {"id":"a","name":"Pair A left"},
        {"id":"b","name":"Pair B left"},
        {"id":"c","name":"Pair A right"},
        {"id":"d","name":"Pair B right"}
    ]))
    .unwrap();
    let decisions = [("a", "c", "related"), ("b", "d", "different")]
        .into_iter()
        .map(|(left, right, choice)| Decision {
            left: left.into(),
            right: right.into(),
            accepted: false,
            method: "jev".into(),
            error: None,
            answer: serde_json::json!({"choice":choice,"confidence":0.99}),
        })
        .collect();
    let report = Report {
        catalog_revision: None,
        dry_run: false,
        model: "test".into(),
        threshold: 0.98,
        candidates: 2,
        errors: 0,
        merchants_scanned: 4,
        merchants,
        groups: vec![],
        decisions,
        run_id: None,
        details_truncated: false,
    };
    let table = render_dedupe(&report);
    let positions: Vec<_> = ["Pair A left", "Pair A right", "Pair B left", "Pair B right"]
        .iter()
        .map(|name| table.find(name).unwrap())
        .collect();
    assert!(positions.windows(2).all(|p| p[0] < p[1]));
    assert!(table.contains("Jev: related"));
    assert!(table.contains("Jev: different"));
    assert!(table.contains("Kept separate"));
}
