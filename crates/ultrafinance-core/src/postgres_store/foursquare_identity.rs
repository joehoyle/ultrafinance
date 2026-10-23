//! Foursquare retains current state and provider identity, not source snapshots.
use super::*;

pub(super) fn persist_initial(
    tx: &mut Transaction<'_>,
    records: &[SourceRecord],
    owners: &[i64],
) -> Result<()> {
    if records.len() != owners.len() {
        bail!("Foursquare identity count mismatch");
    }
    foursquare_locations::import_owned(tx, records, owners, true)?;
    foursquare_categories::persist(tx, &foursquare_categories::assignments(records))?;
    let keys: Vec<_> = records.iter().map(|r| r.external_id.clone()).collect();
    tx.execute(
        "INSERT INTO initial_identity_owners SELECT * FROM unnest($1::text[],$2::bigint[])",
        &[&keys, &owners],
    )?;
    let mut missing_keys = Vec::new();
    let mut missing_owners = Vec::new();
    for (record, owner) in records.iter().zip(owners) {
        if !crate::foursquare::locations(record)?
            .iter()
            .any(|l| l.external_id == record.external_id)
        {
            missing_keys.push(record.external_id.clone());
            missing_owners.push(*owner);
        }
    }
    tx.execute(
        "INSERT INTO foursquare_unlocated SELECT * FROM unnest($1::text[],$2::bigint[])",
        &[&missing_keys, &missing_owners],
    )?;
    Ok(())
}

#[derive(Default)]
struct OwnerState {
    old: HashMap<String, String>,
    before: HashMap<String, Merchant>,
    manual: HashSet<String>,
    replaceable: HashSet<String>,
    numeric: HashMap<String, i64>,
}

impl OwnerState {
    fn load(&mut self, tx: &mut Transaction<'_>, owners: &str, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let sql = format!("{owners} {}", include_str!("foursquare_merchant_state.sql"));
        for row in tx.query(&sql, &[&ids])? {
            let merchant = rows::merchant(&row)?;
            let id = merchant.id.clone();
            for external_id in row.get::<_, Vec<String>>("external_ids") {
                self.old.insert(external_id, id.clone());
            }
            if row.get("is_manual") {
                self.manual.insert(id.clone());
            }
            if row.get("replaceable") {
                self.replaceable.insert(id.clone());
            }
            self.numeric.insert(id.clone(), row.get("merchant_key"));
            self.before.insert(id, merchant);
        }
        Ok(())
    }
}

// Bump this whenever Foursquare reconciliation semantics change. Import mode is
// encoded by the raw places payload (removed for merchants-only imports).
const FINGERPRINT_VERSION: u32 = 1;

fn fingerprint(record: &SourceRecord) -> Result<Vec<u8>> {
    use sha2::{Digest, Sha256};
    // Dataset snapshot/version metadata is not applied by this importer. Exclude
    // it so an unchanged place still skips when a new export is downloaded.
    let input = serde_json::to_vec(&(
        FINGERPRINT_VERSION,
        &record.source,
        &record.external_id,
        &record.merchant,
        &record.attribution,
        &record.license,
        &record.url,
        &record.raw,
    ))?;
    Ok(Sha256::digest(input).to_vec())
}

pub(super) fn import(
    tx: &mut Transaction<'_>,
    records: &[SourceRecord],
) -> Result<(super::super::ImportDelta, Vec<(Merchant, Merchant)>)> {
    if records.is_empty() {
        return Ok((Default::default(), vec![]));
    }
    let mut seen = HashSet::new();
    for record in records {
        validate(&record.merchant)?;
        if record.source != "foursquare"
            || record.external_id.trim().is_empty()
            || !seen.insert(&record.external_id)
        {
            bail!("Foursquare imports require unique nonblank provider IDs");
        }
    }
    let ids: Vec<_> = records.iter().map(|r| r.external_id.clone()).collect();
    let fingerprints: Vec<_> = records.iter().map(fingerprint).collect::<Result<_>>()?;
    let cached: HashMap<String, Vec<u8>> = tx.query(
        "SELECT f.external_id,f.fingerprint FROM unnest($1::text[]) i(id) CROSS JOIN LATERAL (SELECT external_id,fingerprint FROM foursquare_import_fingerprints WHERE external_id=i.id OFFSET 0) f",
        &[&ids],
    )?.into_iter().map(|r| (r.get(0), r.get(1))).collect();
    let mut pending = Vec::new();
    let mut pending_hashes = Vec::new();
    for (record, hash) in records.iter().zip(fingerprints) {
        if cached.get(&record.external_id) != Some(&hash) {
            pending.push(record.clone());
            pending_hashes.push(hash);
        }
    }
    let skipped = records.len() - pending.len();
    if pending.is_empty() {
        return Ok((
            super::super::ImportDelta {
                unchanged: skipped,
                ..Default::default()
            },
            vec![],
        ));
    }
    let (mut delta, merges, owners) = reconcile(tx, &pending)?;
    let ids: Vec<_> = pending.iter().map(|r| r.external_id.clone()).collect();
    let owners: Vec<_> = ids.iter().map(|id| owners[id]).collect();
    // All reconciliation/invalidation has finished. Saving only the records
    // actually processed avoids revalidating cached siblings changed indirectly.
    tx.execute("INSERT INTO foursquare_import_fingerprints SELECT * FROM unnest($1::text[],$2::bigint[],$3::bytea[]) ON CONFLICT(external_id) DO UPDATE SET merchant_id=excluded.merchant_id,fingerprint=excluded.fingerprint", &[&ids,&owners,&pending_hashes])?;
    delta.unchanged += skipped;
    Ok((delta, merges))
}

type Reconciled = (
    super::super::ImportDelta,
    Vec<(Merchant, Merchant)>,
    HashMap<String, i64>,
);

fn reconcile(tx: &mut Transaction<'_>, records: &[SourceRecord]) -> Result<Reconciled> {
    let mut records = records.to_vec();
    for record in &mut records {
        crate::foursquare::hydrate_categories(record);
    }
    let records = records.as_slice();
    let mut seen = HashSet::new();
    for r in records {
        validate(&r.merchant)?;
        if r.source != "foursquare"
            || r.external_id.trim().is_empty()
            || !seen.insert(&r.external_id)
        {
            bail!("Foursquare imports require unique nonblank provider IDs");
        }
    }
    let keys: Vec<_> = records.iter().map(|r| r.external_id.clone()).collect();
    let mut state = OwnerState::default();
    state.load(tx, include_str!("foursquare_identity_lookup.sql"), &keys)?;
    let matching = crate::import_progress::Progress::new(
        "matching new merchant identities",
        records.len() - state.old.len(),
    );
    let rules: std::collections::BTreeSet<_> = records
        .iter()
        .filter(|r| !state.old.contains_key(&r.external_id))
        .filter_map(|r| crate::dedupe::deterministic_key(&r.merchant))
        .map(|(n, h)| (n, h.unwrap_or_default()))
        .collect();
    let names: Vec<_> = rules.iter().map(|k| k.0.clone()).collect();
    let hosts: Vec<_> = rules.iter().map(|k| k.1.clone()).collect();
    let anchors: HashMap<(String,String),(i64,Option<String>)> = tx.query("SELECT i.name,i.host,(SELECT count(*) FROM (SELECT 1 FROM merchant_identity_keys k JOIN manual_merchants m ON merchant_key(m.id)=k.merchant_id WHERE k.rule_name=i.name AND k.rule_host=i.host LIMIT 2) manuals),COALESCE((SELECT merchant_public_id(k.merchant_id) FROM merchant_identity_keys k JOIN manual_merchants m ON merchant_key(m.id)=k.merchant_id WHERE k.rule_name=i.name AND k.rule_host=i.host LIMIT 1),(SELECT merchant_public_id(merchant_id) FROM merchant_identity_keys WHERE rule_name=i.name AND rule_host=i.host ORDER BY merchant_public_id(merchant_id) LIMIT 1)) FROM unnest($1::text[],$2::text[]) i(name,host)", &[&names,&hosts])?.iter().map(|r|((r.get(0),r.get(1)),(r.get(2),r.get(3)))).collect();
    let mut owners = Vec::with_capacity(records.len());
    let mut buckets: HashMap<(String, String), String> = HashMap::new();
    for r in records {
        if let Some(id) = state.old.get(&r.external_id) {
            owners.push(id.clone());
            continue;
        }
        let fresh = || format!("mer_{}", uuid::Uuid::new_v4().simple());
        let id = if let Some((name, host)) = crate::dedupe::deterministic_key(&r.merchant) {
            let key = (name, host.unwrap_or_default());
            let (manuals, target) = &anchors[&key];
            if *manuals > 1 {
                fresh()
            } else {
                buckets
                    .entry(key)
                    .or_insert_with(|| target.clone().unwrap_or_else(fresh))
                    .clone()
            }
        } else {
            fresh()
        };
        owners.push(id);
    }
    matching.finish();
    let mut writing =
        crate::import_progress::Progress::new("writing current Foursquare records", records.len());
    // New places may match an existing merchant through the deterministic rule.
    // Load only those owners not already returned by the provider-ID lookup.
    let missing: Vec<_> = owners
        .iter()
        .filter(|id| !state.before.contains_key(*id))
        .cloned()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    state.load(tx, "WITH owners AS (SELECT m.merchant_key,ARRAY[]::text[] AS external_ids FROM unnest($1::text[]) i(id) CROSS JOIN LATERAL (SELECT merchant_key FROM merchants WHERE id=i.id OFFSET 0) m)", &missing)?;
    let OwnerState {
        old,
        before,
        manual,
        replaceable,
        mut numeric,
    } = state;
    let mut current = before.clone();
    for (r, id) in records.iter().zip(&owners) {
        let merchant = current.entry(id.clone()).or_insert_with(|| {
            let mut m = r.merchant.clone();
            m.id = id.clone();
            m
        });
        // Categories are chosen at creation, not projected from all owned places.
        let categories = std::mem::take(&mut merchant.category_ids);
        // Current merchant state is authoritative; imports add current names and
        // coverage, without retaining per-place merchant evidence for replay.
        if !manual.contains(id) {
            // A single provider-owned merchant can be replaced directly. Shared
            // merchants keep canonical fields while combining new contributions.
            if old.contains_key(&r.external_id) && replaceable.contains(id) {
                *merchant = r.merchant.clone();
                merchant.id = id.clone();
            }
            import_bulk::combine_source(merchant, &r.merchant, &r.url);
        }
        merchant.category_ids = categories;
        import_bulk::finish_merchant(merchant, id);
    }
    // Prepare each outlet once, then compare typed fields against the stored row.
    let mut preparing =
        crate::import_progress::Progress::new("preparing place locations", records.len());
    let mut prepared = Vec::with_capacity(records.len());
    for (index, record) in records.iter().enumerate() {
        prepared.push(crate::foursquare::locations(record)?);
        preparing.advance(index + 1);
    }
    preparing.finish();
    let addressed: usize = prepared.iter().map(Vec::len).sum();
    let places: usize = records
        .iter()
        .map(|r| r.raw["places"].as_array().map_or(0, Vec::len))
        .sum();
    crate::import_progress::locations(addressed, places.saturating_sub(addressed));
    let location_keys: Vec<_> = prepared
        .iter()
        .flatten()
        .map(|r| r.external_id.clone())
        .collect();
    let (existing_outlets, unlocated) = if location_keys.is_empty() {
        (HashMap::new(), HashSet::new())
    } else {
        let outlets: HashMap<String, LocationRecord> = tx
            .query(
                &format!(
                    "{} WHERE l.source='foursquare' AND l.external_id=ANY($1)",
                    rows::LOCATIONS
                ),
                &[&location_keys],
            )?
            .iter()
            .map(|r| Ok((r.get("external_id"), rows::location(r)?)))
            .collect::<Result<_>>()?;
        let unlocated: HashSet<String> = tx
            .query(
                "SELECT external_id FROM foursquare_unlocated WHERE external_id=ANY($1)",
                &[&location_keys],
            )?
            .iter()
            .map(|r| r.get(0))
            .collect();
        (outlets, unlocated)
    };
    let category_records = foursquare_categories::changes(tx, records)?;
    let category_keys: HashSet<_> = category_records
        .iter()
        .map(|r| r.identity_external_id.as_deref().unwrap_or(&r.external_id))
        .collect();
    let mut changed_outlets = Vec::new();
    let mut outlet_keys = HashSet::new();
    let mut country_owners = HashSet::new();
    let mut new_unlocated = Vec::new();
    let mut now_located = Vec::new();
    for ((record, id), outlets) in records.iter().zip(&owners).zip(&prepared) {
        if before
            .get(id)
            .is_none_or(|old| old.markets != current[id].markets)
        {
            country_owners.insert(id.clone());
        }
        if !old.contains_key(&record.external_id)
            && !outlets
                .iter()
                .any(|outlet| outlet.external_id == record.external_id)
        {
            new_unlocated.push((record.external_id.clone(), id.clone()));
        }
        for outlet in outlets {
            let previous = existing_outlets.get(&outlet.external_id);
            if previous.is_some_and(|old| old.manual_override) {
                continue;
            }
            if previous.is_none_or(|old| !same_outlet(old, outlet, id)) {
                outlet_keys.insert(record.external_id.clone());
                changed_outlets.push(outlet.clone());
            }
            if previous.is_none_or(|old| old.location.country != outlet.location.country) {
                country_owners.insert(id.clone());
            }
            if unlocated.contains(&outlet.external_id) {
                now_located.push(outlet.external_id.clone());
                country_owners.insert(id.clone());
            }
        }
    }
    // New owners must exist before writing their outlet foreign keys. Existing
    // merchants wait until category/country projections are final, for one write.
    let new_merchants: Vec<_> = current
        .values()
        .filter(|m| !before.contains_key(&m.id))
        .cloned()
        .collect();
    import_bulk::write_current_merchants(tx, &new_merchants, &before)?;
    let new_ids: Vec<_> = new_merchants.iter().map(|m| m.id.clone()).collect();
    if !new_ids.is_empty() {
        numeric.extend(numeric_ids::keys(tx, &new_ids)?);
    }
    let outlet_owners: HashMap<_, _> = records
        .iter()
        .zip(&owners)
        .map(|(r, id)| (r.external_id.clone(), numeric[id]))
        .collect();
    foursquare_locations::import_changed(tx, &changed_outlets, &outlet_owners)?;
    if !new_unlocated.is_empty() {
        let keys: Vec<_> = new_unlocated.iter().map(|(key, _)| key.clone()).collect();
        let owners: Vec<_> = new_unlocated.iter().map(|(_, id)| numeric[id]).collect();
        tx.execute(
            "INSERT INTO foursquare_unlocated SELECT * FROM unnest($1::text[],$2::bigint[])",
            &[&keys, &owners],
        )?;
    }
    if !now_located.is_empty() {
        tx.execute(
            "DELETE FROM foursquare_unlocated WHERE external_id=ANY($1)",
            &[&now_located],
        )?;
    }
    foursquare_categories::persist(tx, &category_records)?;
    refresh_countries(tx, &mut current, country_owners.into_iter().collect())?;
    let changed: Vec<_> = current
        .values()
        .filter(|m| before.get(&m.id) != Some(m))
        .cloned()
        .collect();
    let changed_ids: HashSet<_> = changed.iter().map(|m| m.id.as_str()).collect();
    let mut delta = super::super::ImportDelta::default();
    for (r, id) in records.iter().zip(&owners) {
        if !old.contains_key(&r.external_id) {
            delta.added += 1;
        } else if changed_ids.contains(id.as_str())
            || outlet_keys.contains(&r.external_id)
            || category_keys.contains(r.external_id.as_str())
        {
            delta.updated += 1;
        } else {
            delta.unchanged += 1;
        }
    }
    let mut known: HashSet<_> = before.keys().cloned().collect();
    // Newly inserted merchants already have their initial projections. Compare
    // final country changes against that persisted state, not against absence.
    let mut persisted = before;
    persisted.extend(new_merchants.into_iter().map(|m| (m.id.clone(), m)));
    import_bulk::write_current_merchants(tx, &changed, &persisted)?;
    let mut merges = Vec::new();
    for (record, id) in records.iter().zip(&owners) {
        if old.contains_key(&record.external_id) {
            continue;
        }
        if !known.insert(id.clone()) {
            let mut incoming = record.merchant.clone();
            incoming.id = format!("mer_{}", uuid::Uuid::new_v4().simple());
            merges.push((current[id].clone(), incoming));
        }
    }
    writing.advance(records.len());
    writing.finish();
    let place_owners = records
        .iter()
        .zip(&owners)
        .map(|(r, id)| (r.external_id.clone(), numeric[id]))
        .collect();
    Ok((delta, merges, place_owners))
}

fn same_outlet(before: &LocationRecord, after: &LocationRecord, owner: &str) -> bool {
    let mut location = after.location.clone();
    location.id = before.location.id.clone();
    matches!(&before.merchant, crate::location::MerchantReference::Local { merchant_id } if merchant_id == owner)
        && before.location == location
        && before.aliases == after.aliases
        && before.transaction_pattern == after.transaction_pattern
        && before.attribution == after.attribution
        && before.license == after.license
        && before.url == after.url
}

fn refresh_countries(
    tx: &mut Transaction<'_>,
    current: &mut HashMap<String, Merchant>,
    ids: Vec<String>,
) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    // Addressless identities deliberately retain their declared merchant markets.
    let located: Vec<String> = tx.query("SELECT i.id FROM (SELECT DISTINCT unnest($1::text[]) id) i WHERE NOT EXISTS(SELECT 1 FROM foursquare_unlocated u WHERE u.merchant_id=merchant_key(i.id))", &[&ids])?.iter().map(|r|r.get(0)).collect();
    if located.is_empty() {
        return Ok(());
    }
    let mut coverage = MarketCountries::default();
    for row in tx.query(
        "SELECT *,merchant_category_ids(id,true) AS category_ids FROM manual_merchants WHERE id=ANY($1)",
        &[&located],
    )? {
        coverage.declaration(row.get("id"), &rows::merchant(&row)?);
    }
    for row in tx.query("SELECT s.*,source_category_ids(s.source,s.external_id) AS input_category_ids,p.attribution,p.license,p.url FROM source_records s JOIN source_provenance p ON p.id=s.source_provenance_id WHERE merchant_id=ANY($1)", &[&located])? {
        let record = rows::source(&row)?;
        let hints: Vec<String> = row.get("country_hints");
        coverage.source_fields(row.get("merchant_id"),&record.merchant,&hints,crate::markets::dataset_region(&record).as_deref());
    }
    postgres_market_outlets(tx, &mut coverage, Some(&located))?;
    for id in located {
        let merchant = current
            .get_mut(&id)
            .context("Foursquare merchant missing")?;
        merchant.markets = coverage.hydrate(merchant.clone()).markets;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/postgres_store/foursquare_identity_tests.rs"]
mod tests;
