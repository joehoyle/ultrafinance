//! Conservative outlet identity, separate from customer-facing brand identity.
use crate::{
    location::{LocationProvenance, LocationRecord},
    store::normalize,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS location_redirects(retired_id TEXT PRIMARY KEY REFERENCES location_records(id), location_id TEXT NOT NULL REFERENCES location_records(id));";

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
    // Contradictions veto all rules. Distinct IDs from one provider remain
    // distinct places. Address
    // normalization preserves unit/suite numbers; neighboring outlets stay apart.
    if conflict(&a_loc.country, &b_loc.country)
        || conflict(&a_loc.region, &b_loc.region)
        || conflict(&a_loc.city, &b_loc.city)
        || conflict(&a_loc.postal_code, &b_loc.postal_code)
        || conflict(&a_loc.address, &b_loc.address)
        || conflict(&a_loc.store_number, &b_loc.store_number)
        || (a.source == b.source && a.external_id != b.external_id)
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
    (same(&a_loc.country, &b_loc.country) && same(&a_loc.store_number, &b_loc.store_number))
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
#[path = "../tests/unit/location_dedupe/tests.rs"]
mod tests;
