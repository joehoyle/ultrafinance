//! Conservative outlet identity, separate from customer-facing brand identity.
use crate::{
    location::{LocationProvenance, LocationRecord},
    store::normalize,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS location_redirects(retired_id TEXT PRIMARY KEY REFERENCES location_records(id), location_id TEXT NOT NULL REFERENCES location_records(id)); CREATE TABLE IF NOT EXISTS location_merge_runs(id TEXT PRIMARY KEY, data TEXT NOT NULL);";

#[derive(Debug, Serialize)]
pub struct Report {
    pub dry_run: bool,
    pub source_records: usize,
    pub locations_before: usize,
    pub locations_after: usize,
    pub groups: Vec<Vec<String>>,
    pub run_id: Option<String>,
}

fn same(a: &Option<String>, b: &Option<String>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => !normalize(a).is_empty() && normalize(a) == normalize(b),
        _ => false,
    }
}
fn conflict(a: &Option<String>, b: &Option<String>) -> bool {
    matches!((a, b), (Some(_), Some(_))) && !same(a, b)
}
fn equivalent(a: &LocationRecord, b: &LocationRecord) -> bool {
    let (a_loc, b_loc) = (&a.location, &b.location);
    // Contradictions veto all rules, even a shared provider ID. Address
    // normalization preserves unit/suite numbers; neighboring outlets stay apart.
    if conflict(&a_loc.country, &b_loc.country)
        || conflict(&a_loc.region, &b_loc.region)
        || conflict(&a_loc.city, &b_loc.city)
        || conflict(&a_loc.postal_code, &b_loc.postal_code)
        || conflict(&a_loc.address, &b_loc.address)
        || conflict(&a_loc.store_number, &b_loc.store_number)
        || a.place_ids
            .iter()
            .any(|(provider, id)| b.place_ids.get(provider).is_some_and(|other| id != other))
    {
        return false;
    }
    if let (Some(lat1), Some(lon1), Some(lat2), Some(lon2)) = (
        a_loc.latitude,
        a_loc.longitude,
        b_loc.latitude,
        b_loc.longitude,
    ) {
        let distance = ((lat1 - lat2) * 111320.0)
            .hypot((lon1 - lon2) * 111320.0 * ((lat1 + lat2) / 2.0).to_radians().cos());
        if distance > 100.0 {
            return false;
        }
    }
    let shared_id = a
        .place_ids
        .iter()
        .any(|(provider, id)| !id.trim().is_empty() && b.place_ids.get(provider) == Some(id));
    shared_id
        || (same(&a_loc.country, &b_loc.country) && same(&a_loc.store_number, &b_loc.store_number))
        || (same(&a_loc.country, &b_loc.country)
            && same(&a_loc.city, &b_loc.city)
            && same(&a_loc.address, &b_loc.address))
}

/// Preserve raw source records; display/matching sees one enriched canonical outlet.
pub(crate) fn consolidate(
    records: Vec<LocationRecord>,
    redirects: &BTreeMap<String, String>,
) -> Vec<LocationRecord> {
    let mut groups: BTreeMap<String, Vec<LocationRecord>> = BTreeMap::new();
    for record in records {
        let id = record.location.id.as_ref().expect("stored outlet ID");
        groups
            .entry(redirects.get(id).unwrap_or(id).clone())
            .or_default()
            .push(record);
    }
    groups.into_iter().map(|(id,mut sources)| {
        sources.sort_by_key(|r| (!r.manual_override,r.location.id.as_ref()!=Some(&id),r.source.clone(),r.external_id.clone()));
        let mut result=sources[0].clone();
        result.location.id=Some(id);
        result.provenance=if sources.len()>1 { sources.iter().map(LocationProvenance::from).collect() } else { vec![] };
        for record in &sources[1..] {
            result.aliases.extend(record.aliases.clone());
            for (key,value) in &record.place_ids { result.place_ids.entry(key.clone()).or_insert_with(||value.clone()); }
            macro_rules! fill { ($($field:ident),*) => { $(if result.location.$field.is_none() { result.location.$field=record.location.$field.clone(); })* }; }
            fill!(name,address,city,region,postal_code,country,store_number,latitude,longitude);
        }
        result.aliases.sort();result.aliases.dedup();
        result
    }).collect()
}

pub(crate) fn plan(
    records: &[(String, LocationRecord)],
    redirects: &BTreeMap<String, String>,
    dry_run: bool,
) -> Report {
    let mut identities: BTreeMap<String, (String, Vec<&LocationRecord>)> = BTreeMap::new();
    for (merchant, record) in records {
        let id = record.location.id.as_ref().expect("stored outlet ID");
        let canonical = redirects.get(id).unwrap_or(id).clone();
        let entry = identities
            .entry(canonical)
            .or_insert_with(|| (merchant.clone(), vec![]));
        entry.1.push(record);
    }
    let ids: Vec<_> = identities.keys().cloned().collect();
    let mut blocks: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    for (i, id) in ids.iter().enumerate() {
        let (merchant, sources) = &identities[id];
        for r in sources {
            for (provider, place) in &r.place_ids {
                if !place.trim().is_empty() {
                    blocks
                        .entry(format!("{merchant}:place:{provider}:{place}"))
                        .or_default()
                        .insert(i);
                }
            }
            if let (Some(country), Some(number)) = (&r.location.country, &r.location.store_number) {
                blocks
                    .entry(format!("{merchant}:store:{country}:{}", normalize(number)))
                    .or_default()
                    .insert(i);
            }
            if let (Some(country), Some(city), Some(address)) =
                (&r.location.country, &r.location.city, &r.location.address)
            {
                blocks
                    .entry(format!(
                        "{merchant}:address:{country}:{}:{}",
                        normalize(city),
                        normalize(address)
                    ))
                    .or_default()
                    .insert(i);
            }
        }
    }
    let mut candidates = BTreeSet::new();
    for block in blocks.values() {
        let values: Vec<_> = block.iter().copied().collect();
        for (at, &a) in values.iter().enumerate() {
            for &b in &values[at + 1..] {
                candidates.insert((a, b));
            }
        }
    }
    let compatible = |a: usize, b: usize| {
        let (ma, sa) = &identities[&ids[a]];
        let (mb, sb) = &identities[&ids[b]];
        ma == mb && sa.iter().all(|a| sb.iter().all(|b| equivalent(a, b)))
    };
    let mut groups: Vec<Vec<usize>> = (0..ids.len()).map(|i| vec![i]).collect();
    for (a, b) in candidates {
        if !compatible(a, b) {
            continue;
        }
        let ga = groups.iter().position(|g| g.contains(&a)).unwrap();
        let gb = groups.iter().position(|g| g.contains(&b)).unwrap();
        if ga == gb {
            continue;
        }
        let manual = groups[ga]
            .iter()
            .chain(&groups[gb])
            .filter(|&&i| identities[&ids[i]].1.iter().any(|r| r.manual_override))
            .count();
        if manual > 1
            || !groups[ga]
                .iter()
                .all(|&a| groups[gb].iter().all(|&b| compatible(a, b)))
        {
            continue;
        }
        let other = groups.remove(ga.max(gb));
        groups[ga.min(gb)].extend(other);
    }
    let mut merged: Vec<Vec<String>> = groups
        .into_iter()
        .filter(|g| g.len() > 1)
        .map(|g| g.into_iter().map(|i| ids[i].clone()).collect())
        .collect();
    for group in &mut merged {
        group.sort_by_key(|id| {
            (
                !identities[id].1.iter().any(|r| r.manual_override),
                std::cmp::Reverse(identities[id].1.len()),
                id.clone(),
            )
        });
    }
    let retired: usize = merged.iter().map(|g| g.len() - 1).sum();
    Report {
        dry_run,
        source_records: records.len(),
        locations_before: ids.len(),
        locations_after: ids.len() - retired,
        groups: merged,
        run_id: None,
    }
}

#[cfg(test)]
mod tests {
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
    fn conflicting_branches_coordinates_provider_ids_and_manual_records_stay_separate() {
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
        let mut identified = a.clone();
        identified.place_ids.insert("fsq".into(), "fsq-a".into());
        b.place_ids.insert("fsq".into(), "fsq-b".into());
        assert!(!equivalent(&identified, &b));
        b.place_ids.clear();
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
        let before = db.fingerprint()?;
        let preview = db.dedupe_locations(Some("brand"), true)?;
        assert!(preview.dry_run);
        assert!(preview.groups.is_empty());
        assert_eq!(before, db.fingerprint()?);
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
        db.apply_dedupe(
            &snapshot,
            &[vec![first.clone(), second.clone()]],
            &json!({"test":true}),
        )?;
        assert_eq!(db.locations(&first)?.len(), 1);
        assert_eq!(db.locations(&second)?.len(), 1);
        assert_eq!(db.location_sources(&first)?.len(), 2);
        assert_eq!(db.locations(&first)?[0].provenance.len(), 2);
        db.import_locations(&[outlet("three", "c", &second, "1 Main St")])?;
        assert_eq!(db.locations(&first)?.len(), 1);
        Ok(())
    }
}
