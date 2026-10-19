use super::*;
use crate::{CounterpartyType, store::MerchantStore};
use serde_json::json;

fn format(processor: &str) -> ProcessorFormat {
    serde_json::from_value(json!({
        "id":"example-star", "processor_id":processor,
        "pattern":"(?i:NEX)\\*[ \\t]*(?P<merchant>\\S(?:[^\\r\\n]*\\S)?)",
        "sources":["https://example.test/docs"], "reviewed_on":"2026-10-19", "scope":"Test fixtures"
    }))
    .unwrap()
}

fn store() -> MerchantStore {
    let store = MerchantStore::temporary().unwrap();
    store
        .put(
            &serde_json::from_value(
                json!({"id":"processor","name":"Example Processor","type":"payment_processor"}),
            )
            .unwrap(),
        )
        .unwrap();
    store
}

#[test]
fn formats_validate_regex_captures_and_provenance() {
    let good = format("processor");
    good.validate().unwrap();
    let regex = good.compile().unwrap();
    let found = capture(&regex, "nex* Café Élan").unwrap();
    assert_eq!(found.merchant, "Café Élan");
    assert_eq!(&"nex* Café Élan"[found.start..], "Café Élan");
    for pattern in ["[", "(?=NEX)NEX.*", ".*", "(?P<merchant>.*)"] {
        let mut bad = good.clone();
        bad.pattern = pattern.into();
        assert!(bad.validate().is_err(), "{pattern}");
    }
    let mut bad = good.clone();
    bad.sources.clear();
    assert!(bad.validate().is_err());
    // A syntactically valid regex must still fail closed if its capture is
    // not a nonempty suffix following a prefix.
    let mut non_suffix = good.clone();
    non_suffix.pattern = "NEX(?P<merchant>.*)END".into();
    assert!(capture(&non_suffix.compile().unwrap(), "NEXShopEND").is_none());
    assert!(capture(&regex, "NEX*").is_none());
    let mut bad = good.clone();
    bad.pattern = "X".repeat(4097);
    assert!(bad.validate().is_err());
    let mut other = good.clone();
    other.processor_id = "another".into();
    assert!(validate_set(&[good, other]).is_err());
}

#[test]
fn format_writes_are_atomic_previewable_and_type_checked() {
    let store = store();
    let good = format("processor");
    store
        .save_processor_formats(vec![good.clone()], true)
        .unwrap();
    assert!(store.processor_formats().unwrap().is_empty());
    let mut missing = good.clone();
    missing.id = "missing".into();
    missing.processor_id = "missing".into();
    assert!(
        store
            .save_processor_formats(vec![good.clone(), missing], false)
            .is_err()
    );
    assert!(store.processor_formats().unwrap().is_empty());
    store
        .put(&serde_json::from_value(json!({"id":"shop","name":"Shop"})).unwrap())
        .unwrap();
    let mut wrong = good.clone();
    wrong.processor_id = "shop".into();
    assert!(store.save_processor_formats(vec![wrong], false).is_err());
    store
        .save_processor_formats(vec![good.clone()], false)
        .unwrap();
    assert_eq!(store.processor_formats().unwrap().len(), 1);
    assert_eq!(
        store
            .processor_format_matches(&["NEX*Alpha Cafe".into()])
            .unwrap()[0][0]
            .processor
            .counterparty_type,
        CounterpartyType::PaymentProcessor
    );
    let mut disabled = good;
    disabled.enabled = false;
    store.save_processor_formats(vec![disabled], false).unwrap();
    assert!(
        store
            .processor_format_matches(&["NEX*Alpha Cafe".into()])
            .unwrap()[0]
            .is_empty()
    );
}

#[test]
fn format_cache_tracks_commits_from_other_connections() {
    let store = store();
    let rule = format("processor");
    store
        .save_processor_formats(vec![rule.clone()], false)
        .unwrap();
    let descriptions = vec!["NEX*Alpha Cafe".to_owned()];
    assert_eq!(
        store.processor_format_matches(&descriptions).unwrap()[0].len(),
        1
    );
    let other = MerchantStore::postgres(store.temporary_url()).unwrap();
    let mut disabled = rule;
    disabled.enabled = false;
    other
        .save_processor_formats(vec![disabled.clone()], true)
        .unwrap();
    assert_eq!(
        store.processor_format_matches(&descriptions).unwrap()[0].len(),
        1
    );
    other
        .put(
            &serde_json::from_value(
                json!({"id":"processor","name":"Renamed Processor","type":"payment_processor"}),
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        store.processor_format_matches(&descriptions).unwrap()[0][0]
            .processor
            .name,
        "Renamed Processor"
    );
    other.save_processor_formats(vec![disabled], false).unwrap();
    assert!(store.processor_format_matches(&descriptions).unwrap()[0].is_empty());
    let mut client = postgres::Client::connect(
        store.temporary_url(),
        postgres_native_tls::MakeTlsConnector::new(
            native_tls::TlsConnector::builder().build().unwrap(),
        ),
    )
    .unwrap();
    client
        .execute("UPDATE processor_formats SET enabled=true", &[])
        .unwrap();
    assert_eq!(
        store.processor_format_matches(&descriptions).unwrap()[0].len(),
        1
    );
}

#[test]
fn researched_seed_formats_validate_without_database_specific_ids() {
    let values: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../../../data/processor-formats/researched.json"
    ))
    .unwrap();
    let formats: Vec<_> = values
        .into_iter()
        .map(|mut value| {
            let name = value.as_object_mut().unwrap().remove("processor").unwrap();
            value["processor_id"] = name;
            serde_json::from_value::<ProcessorFormat>(value).unwrap()
        })
        .collect();
    super::validate_set(&formats).unwrap();

    #[derive(serde::Deserialize)]
    struct Example {
        description: String,
        merchant: String,
    }
    #[derive(serde::Deserialize)]
    struct Fixture {
        id: String,
        positive_examples: Vec<Example>,
        negative_examples: Vec<String>,
    }
    let fixtures: Vec<Fixture> =
        serde_json::from_str(include_str!("../../fixtures/processor_formats.json")).unwrap();
    assert_eq!(fixtures.len(), formats.len());
    let mut tested = std::collections::HashSet::new();
    for fixture in fixtures {
        assert!(tested.insert(fixture.id.clone()), "duplicate fixture");
        let owner = formats
            .iter()
            .find(|f| f.id == fixture.id)
            .expect("fixture rule exists");
        assert!(fixture.positive_examples.len() >= 2);
        assert!(fixture.negative_examples.len() >= 2);
        let regex = owner.compile().unwrap();
        for example in fixture.positive_examples {
            let found = capture(&regex, &example.description).expect("positive fixture matches");
            assert_eq!(found.merchant, example.merchant, "{}", fixture.id);
            // Catch conflicting matches against the complete researched catalog.
            for other in &formats {
                if other.enabled
                    && let Some(overlap) = capture(&other.compile().unwrap(), &example.description)
                {
                    assert_eq!(
                        other.processor_id, owner.processor_id,
                        "{} overlaps {}",
                        owner.id, other.id
                    );
                    assert_eq!(overlap.merchant, example.merchant);
                }
            }
        }
        for example in fixture.negative_examples {
            assert!(
                capture(&regex, &example).is_none(),
                "{} matched {example:?}",
                fixture.id
            );
        }
    }
}
