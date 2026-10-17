//! Foursquare retains current state and provider identity, not source snapshots.
use super::*;

pub(super) fn persist(
    tx: &mut Transaction<'_>,
    records: &[SourceRecord],
    owners: &[i64],
    initial: bool,
) -> Result<()> {
    if records.len() != owners.len() {
        bail!("Foursquare identity count mismatch");
    }
    // Write outlets first, then keep only IDs that have no outlet. The transaction
    // and import write lock make transitions between these representations atomic.
    foursquare_locations::import_owned(tx, records, owners, initial)?;
    let keys: Vec<_> = records.iter().map(|r| r.external_id.clone()).collect();
    if initial {
        // Initial outlet indexes are deferred: never probe the growing heap.
        // Record the plan and validate ownership once after indexes are rebuilt.
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
        return Ok(());
    }
    tx.execute("INSERT INTO foursquare_unlocated SELECT i.external_id,i.merchant_id FROM unnest($1::text[],$2::bigint[]) i(external_id,merchant_id) WHERE NOT EXISTS (SELECT 1 FROM location_records l WHERE l.source='foursquare' AND l.external_id=i.external_id) ON CONFLICT(external_id) DO UPDATE SET merchant_id=excluded.merchant_id WHERE foursquare_unlocated.merchant_id IS DISTINCT FROM excluded.merchant_id", &[&keys,&owners])?;
    tx.execute("DELETE FROM foursquare_unlocated u USING unnest($1::text[]) i(external_id) WHERE u.external_id=i.external_id AND EXISTS(SELECT 1 FROM location_records l WHERE l.source='foursquare' AND l.external_id=i.external_id)", &[&keys])?;
    let bad: bool = tx.query_one("SELECT EXISTS(SELECT 1 FROM unnest($1::text[],$2::bigint[]) i(external_id,merchant_id) WHERE (SELECT count(*) FROM source_identities s WHERE s.source='foursquare' AND s.external_id=i.external_id AND s.merchant_id=merchant_public_id(i.merchant_id))<>1)", &[&keys,&owners])?.get(0);
    if bad {
        bail!("Foursquare import identity ownership does not match its merchant");
    }
    Ok(())
}

pub(super) fn import(
    tx: &mut Transaction<'_>,
    records: &[SourceRecord],
) -> Result<(super::super::ImportDelta, Vec<(Merchant, Merchant)>)> {
    if records.is_empty() {
        return Ok((Default::default(), vec![]));
    }
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
    let old: HashMap<String,String> = tx.query("SELECT s.external_id,s.merchant_id FROM unnest($1::text[]) i(external_id) CROSS JOIN LATERAL (SELECT merchant_id,external_id FROM source_identities s WHERE s.source='foursquare' AND s.external_id=i.external_id OFFSET 0) s", &[&keys])?.iter().map(|r|(r.get(0),r.get(1))).collect();
    let matching = crate::import_progress::Progress::new(
        "matching new merchant identities",
        records.len() - old.len(),
    );
    let rules: std::collections::BTreeSet<_> = records
        .iter()
        .filter(|r| !old.contains_key(&r.external_id))
        .filter_map(|r| crate::dedupe::deterministic_key(&r.merchant))
        .map(|(n, h)| (n, h.unwrap_or_default()))
        .collect();
    let names: Vec<_> = rules.iter().map(|k| k.0.clone()).collect();
    let hosts: Vec<_> = rules.iter().map(|k| k.1.clone()).collect();
    let anchors: HashMap<(String,String),(i64,Option<String>)> = tx.query("SELECT i.name,i.host,(SELECT count(*) FROM (SELECT 1 FROM merchant_identity_keys k JOIN manual_merchants m ON merchant_key(m.id)=k.merchant_id WHERE k.rule_name=i.name AND k.rule_host=i.host LIMIT 2) manuals),COALESCE((SELECT merchant_public_id(k.merchant_id) FROM merchant_identity_keys k JOIN manual_merchants m ON merchant_key(m.id)=k.merchant_id WHERE k.rule_name=i.name AND k.rule_host=i.host LIMIT 1),(SELECT merchant_public_id(merchant_id) FROM merchant_identity_keys WHERE rule_name=i.name AND rule_host=i.host ORDER BY merchant_public_id(merchant_id) LIMIT 1)) FROM unnest($1::text[],$2::text[]) i(name,host)", &[&names,&hosts])?.iter().map(|r|((r.get(0),r.get(1)),(r.get(2),r.get(3)))).collect();
    let mut owners = Vec::with_capacity(records.len());
    let mut buckets: HashMap<(String, String), String> = HashMap::new();
    for r in records {
        if let Some(id) = old.get(&r.external_id) {
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
    let before: HashMap<String, Merchant> = tx
        .query("SELECT * FROM merchants WHERE id=ANY($1)", &[&owners])?
        .iter()
        .map(|r| Ok((r.get("id"), rows::merchant(r)?)))
        .collect::<Result<_>>()?;
    let manual: HashSet<String> = tx
        .query(
            "SELECT id FROM manual_merchants WHERE id=ANY($1)",
            &[&owners],
        )?
        .iter()
        .map(|r| r.get(0))
        .collect();
    let replaceable: HashSet<String> = tx.query("SELECT i.id FROM (SELECT DISTINCT unnest($1::text[]) AS id) i WHERE (SELECT count(*) FROM (SELECT 1 FROM source_identities s WHERE s.merchant_id=i.id LIMIT 2) refs)=1", &[&owners])?.iter().map(|r|r.get(0)).collect();
    let mut current = before.clone();
    for (r, id) in records.iter().zip(&owners) {
        let merchant = current.entry(id.clone()).or_insert_with(|| {
            let mut m = r.merchant.clone();
            m.id = id.clone();
            m
        });
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
        import_bulk::finish_merchant(merchant, id);
    }
    let changed: Vec<_> = current
        .values()
        .filter(|m| {
            before
                .get(&m.id)
                .is_none_or(|old| serde_json::to_value(old).ok() != serde_json::to_value(m).ok())
        })
        .cloned()
        .collect();
    let changed_ids: HashSet<_> = changed.iter().map(|m| m.id.as_str()).collect();
    let outlet_keys: Vec<_> = records
        .iter()
        .map(crate::foursquare::locations)
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .map(|l| l.external_id)
        .collect();
    let existing_outlets: HashMap<String, LocationRecord> = tx
        .query(
            &format!(
                "{} WHERE l.source='foursquare' AND l.external_id=ANY($1)",
                rows::LOCATIONS
            ),
            &[&outlet_keys],
        )?
        .iter()
        .map(|r| Ok((r.get("external_id"), rows::location(r)?)))
        .collect::<Result<_>>()?;
    let mut delta = super::super::ImportDelta::default();
    for (r, id) in records.iter().zip(&owners) {
        if !old.contains_key(&r.external_id) {
            delta.added += 1;
            continue;
        }
        let mut different = changed_ids.contains(id.as_str());
        for mut outlet in crate::foursquare::locations(r)? {
            if let Some(before) = existing_outlets.get(&outlet.external_id) {
                if before.manual_override {
                    continue;
                }
                outlet.location.id = before.location.id.clone();
                outlet.merchant = crate::location::MerchantReference::Local {
                    merchant_id: id.clone(),
                };
                different |= serde_json::to_value(before)? != serde_json::to_value(outlet)?;
            } else {
                different = true;
            }
        }
        if different {
            delta.updated += 1;
        } else {
            delta.unchanged += 1;
        }
    }
    import_bulk::write_current_merchants(tx, &changed)?;
    let numeric = numeric_ids::keys(tx, &owners)?;
    let owner_keys: Vec<_> = owners.iter().map(|id| numeric[id]).collect();
    persist(tx, records, &owner_keys, false)?;
    // Addressed places carry current country coverage. Addressless identities
    // deliberately keep no country evidence, so retain their merchant coverage.
    let located: Vec<String> = tx.query("SELECT DISTINCT i.id FROM unnest($1::text[]) i(id) WHERE NOT EXISTS(SELECT 1 FROM foursquare_unlocated u WHERE u.merchant_id=merchant_key(i.id))", &[&owners])?.iter().map(|r|r.get(0)).collect();
    let mut coverage = MarketCountries::default();
    for row in tx.query(
        "SELECT * FROM manual_merchants WHERE id=ANY($1)",
        &[&located],
    )? {
        coverage.declaration(row.get("id"), &rows::merchant(&row)?);
    }
    for row in tx.query("SELECT s.*,p.attribution,p.license,p.url FROM source_records s JOIN source_provenance p ON p.id=s.source_provenance_id WHERE merchant_id=ANY($1)", &[&located])? {
        let record = rows::source(&row)?;
        let hints: Vec<String> = row.get("country_hints");
        coverage.source_fields(row.get("merchant_id"),&record.merchant,&hints,crate::markets::dataset_region(&record).as_deref());
    }
    postgres_market_outlets(tx, &mut coverage, Some(&located))?;
    for id in located {
        let merchant = current
            .get_mut(&id)
            .context("Foursquare merchant missing")?;
        let markets = coverage.hydrate(merchant.clone()).markets;
        if merchant.markets != markets {
            tx.execute(
                "UPDATE merchants SET markets=$2 WHERE id=$1",
                &[&id, &markets],
            )?;
            merchant.markets = markets;
        }
    }
    let mut known: HashSet<_> = before.keys().cloned().collect();
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
    Ok((delta, merges))
}

#[cfg(test)]
#[path = "../../tests/unit/postgres_store/foursquare_identity_tests.rs"]
mod tests;
