use super::*;
use crate::{
    location::{LocationResult, MerchantReference},
    store::MerchantStore,
};
use serde_json::json;
fn outlet(source: &str, external: &str, merchant: &str, address: &str) -> LocationRecord {
    serde_json::from_value(json!({"source":source,"external_id":external,"merchant":{"merchant_id":merchant},"location":{"id":external,"precision":"outlet","address":address,"city":"Bromont","region":"QC","postal_code":"J2L 1A1","country":"CA","store_number":null},"aliases":["STARBUCKS BROMONT"],"attribution":source,"license":"test","url":"https://example.test"})).unwrap()
}
#[test]
fn conflicting_branches_coordinates_source_keys_and_manual_records_stay_separate() {
    let a = outlet("one", "a", "brand", "1 Main St Suite 1");
    let mut b = outlet("two", "b", "brand", "1 Main St Suite 2");
    assert!(!equivalent(&a, &b));
    b.location.address = a.location.address.clone();
    b.location.store_number = Some("002".into());
    let mut numbered = a.clone();
    numbered.location.store_number = Some("001".into());
    assert!(!equivalent(&numbered, &b));
    b.location.store_number = None;
    b.location.city = Some("Montreal".into());
    assert!(!equivalent(&a, &b));
    b.location.city = a.location.city.clone();
    let mut distant = a.clone();
    distant.location.latitude = Some(45.0);
    distant.location.longitude = Some(-72.0);
    b.location.latitude = Some(46.0);
    b.location.longitude = Some(-72.0);
    assert!(!equivalent(&distant, &b));
    b.location.latitude = None;
    b.location.longitude = None;
    let mut same_provider = a.clone();
    same_provider.external_id = "different-provider-place".into();
    assert!(!equivalent(&a, &same_provider));
    let mut manual = a.clone();
    manual.manual_override = true;
    b.manual_override = true;
    assert!(
        plan(
            &[("brand".into(), manual), ("brand".into(), b)],
            &Default::default(),
            true
        )
        .groups
        .is_empty()
    );
    let mut no_address = a.clone();
    no_address.location.address = None;
    assert!(!equivalent(&no_address, &no_address));
    assert!(
        plan(
            &[("first".into(), a.clone()), ("second".into(), a)],
            &Default::default(),
            true
        )
        .groups
        .is_empty()
    );
}
#[test]
fn complete_link_rejects_conflicting_evidence_through_a_missing_field() {
    let mut a = outlet("a", "a", "brand", "1 Main St");
    a.location.store_number = Some("001".into());
    let b = outlet("b", "b", "brand", "1 Main St");
    let mut c = outlet("c", "c", "brand", "1 Main St");
    c.location.store_number = Some("002".into());
    let result = plan(
        &[
            ("brand".into(), a),
            ("brand".into(), b),
            ("brand".into(), c),
        ],
        &Default::default(),
        true,
    );
    assert_eq!(result.locations_after, 2);
    assert_eq!(result.groups[0].len(), 2);
}
#[test]
fn imports_consolidate_without_losing_evidence_patterns_or_stable_ids() -> anyhow::Result<()> {
    let db = MerchantStore::temporary()?;
    let merchant = serde_json::from_value(json!({"id":"brand","name":"Starbucks"}))?;
    db.put(&merchant)?;
    let a = outlet("one", "a", "brand", "1 Main St");
    let mut b = outlet("two", "b", "brand", "1 Main St");
    b.aliases = vec!["STARBUCKS STORE 1".into()];
    b.transaction_pattern = Some("^SBROMONT$".into());
    b.location.latitude = Some(45.3);
    b.location.longitude = Some(-72.6);
    db.import_locations(&[a.clone(), b.clone()])?;
    let canonical = db.locations("brand")?;
    assert_eq!(canonical.len(), 1);
    let id = canonical[0].location.id.clone();
    assert_eq!(canonical[0].aliases.len(), 2);
    assert_eq!(canonical[0].provenance.len(), 2);
    assert_eq!(canonical[0].location.latitude, b.location.latitude);
    assert_eq!(db.location_sources("brand")?.len(), 2);
    let request = serde_json::from_value(json!({"description":"SBROMONT","country":"CA"}))?;
    assert!(matches!(
        crate::location::enrich(&request, &canonical).0,
        LocationResult::Matched { .. }
    ));
    let before = serde_json::to_value(db.location_sources("brand")?)?;
    let preview = db.dedupe_locations(Some("brand"), true)?;
    assert!(preview.dry_run);
    assert!(preview.groups.is_empty());
    assert_eq!(before, serde_json::to_value(db.location_sources("brand")?)?);
    db.import_locations(&[a, b.clone()])?;
    assert_eq!(db.locations("brand")?[0].location.id, id);
    let mut correction = b.clone();
    correction.manual_override = true;
    correction.location.name = Some("Reviewed outlet".into());
    db.import_locations(&[correction])?;
    db.import_locations(&[b])?;
    assert_eq!(
        db.locations("brand")?[0].location.name,
        Some("Reviewed outlet".into())
    );
    assert_eq!(db.locations("brand")?[0].location.id, id);
    let separate = outlet("two", "c", "brand", "2 Main St");
    db.import_locations(&[separate])?;
    assert_eq!(db.locations("brand")?.len(), 2);
    Ok(())
}
#[test]
fn merchant_merges_reconcile_source_referenced_outlets() -> anyhow::Result<()> {
    let db = MerchantStore::temporary()?;
    let source = |id: &str| crate::store::SourceRecord {
        source: "brands".into(),
        external_id: id.into(),
        merchant: serde_json::from_value(json!({"id":id,"name":"Starbucks"})).unwrap(),
        attribution: "test".into(),
        license: "test".into(),
        url: "https://example.test".into(),
        version: None,
        raw: json!({}),
    };
    db.import(&[source("a"), source("b")])?;
    let first = db.resolve_source("brands", "a")?.unwrap();
    let second = db.resolve_source("brands", "b")?.unwrap();
    let mut a = outlet("one", "a", &first, "1 Main St");
    let mut b = outlet("two", "b", &second, "1 Main St");
    a.merchant = MerchantReference::Source {
        source: "brands".into(),
        external_id: "a".into(),
    };
    b.merchant = MerchantReference::Source {
        source: "brands".into(),
        external_id: "b".into(),
    };
    db.import_locations(&[a, b])?;
    let snapshot = db.dedupe_snapshot()?;
    assert_eq!(snapshot.locations.len(), 2);
    db.apply_dedupe(&snapshot, &[vec![first.clone(), second.clone()]])?;
    assert_eq!(db.locations(&first)?.len(), 1);
    assert_eq!(db.locations(&second)?.len(), 1);
    assert_eq!(db.location_sources(&first)?.len(), 2);
    assert_eq!(db.locations(&first)?[0].provenance.len(), 2);
    db.import_locations(&[outlet("three", "c", &second, "1 Main St")])?;
    assert_eq!(db.locations(&first)?.len(), 1);
    Ok(())
}
