//! Bounded bulk writes for guarded source imports. All stages belong to the
//! caller's transaction, including merchant search and market countries.
use super::*;
use postgres::{binary_copy::BinaryCopyInWriter, types::Type};

const BATCH: usize = 5000;

fn copy_rows(
    tx: &mut Transaction<'_>,
    sql: &str,
    width: usize,
    rows: &[Vec<Option<String>>],
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let sink = tx.copy_in(sql)?;
    let types = vec![Type::TEXT; width];
    let mut writer = BinaryCopyInWriter::new(sink, &types);
    for row in rows {
        let values: Vec<&(dyn postgres::types::ToSql + Sync)> =
            row.iter().map(|v| v as _).collect();
        writer.write(&values)?;
    }
    writer.finish()?;
    Ok(())
}
fn copy_merchants(tx: &mut Transaction<'_>, merchants: &[Merchant]) -> Result<()> {
    if merchants.is_empty() {
        return Ok(());
    }
    let sink = tx.copy_in(COPY_MERCHANTS)?;
    let mut writer = BinaryCopyInWriter::new(
        sink,
        &[
            Type::TEXT,
            Type::TEXT,
            Type::TEXT,
            Type::TEXT,
            Type::TEXT,
            Type::TEXT_ARRAY,
            Type::TEXT_ARRAY,
            Type::TEXT_ARRAY,
        ],
    );
    for m in merchants {
        writer.write(&[
            &m.id,
            &m.name,
            &m.website,
            &m.logo_url,
            &m.logo_source,
            &m.markets,
            &m.aliases,
            &m.sources,
        ])?;
    }
    writer.finish()?;
    Ok(())
}
const COPY_MERCHANTS: &str = "COPY import_merchant_stage(id,name,website,logo_url,logo_source,markets,aliases,sources) FROM STDIN BINARY";
const UPSERT_MERCHANTS: &str = "INSERT INTO merchants(id,name,website,logo_url,logo_source,markets,aliases,sources) SELECT id,name,website,logo_url,logo_source,markets,aliases,sources FROM import_merchant_stage ON CONFLICT(id) DO UPDATE SET name=excluded.name,website=excluded.website,logo_url=excluded.logo_url,logo_source=excluded.logo_source,markets=excluded.markets,aliases=excluded.aliases,sources=excluded.sources WHERE (merchants.name,merchants.website,merchants.logo_url,merchants.logo_source,merchants.markets,merchants.aliases,merchants.sources) IS DISTINCT FROM (excluded.name,excluded.website,excluded.logo_url,excluded.logo_source,excluded.markets,excluded.aliases,excluded.sources)";

pub(super) fn import(
    tx: &mut Transaction<'_>,
    records: &[SourceRecord],
    identities: &[String],
    baseline: &crate::dedupe::Snapshot,
) -> Result<super::super::ImportDelta> {
    Ok(import_indexed(tx, records, identities, baseline)?.delta)
}

pub(super) struct Applied {
    pub delta: super::super::ImportDelta,
    pub refreshed_keys: HashSet<String>,
}

/// Report which keys were rebuilt so the indexed caller need only refresh
/// unchanged source identities afterwards. Keys are ready before matching new rows.
pub(super) fn import_indexed(
    tx: &mut Transaction<'_>,
    records: &[SourceRecord],
    identities: &[String],
    baseline: &crate::dedupe::Snapshot,
) -> Result<Applied> {
    apply(tx, records, identities, baseline, false)
}

/// The indexed caller has checked that every source identity is new while
/// holding the import write lock. Existing source updates must use import_indexed.
pub(super) fn import_new_sources(
    tx: &mut Transaction<'_>,
    records: &[SourceRecord],
    identities: &[String],
    baseline: &crate::dedupe::Snapshot,
) -> Result<Applied> {
    apply(tx, records, identities, baseline, true)
}

fn apply(
    tx: &mut Transaction<'_>,
    records: &[SourceRecord],
    identities: &[String],
    baseline: &crate::dedupe::Snapshot,
    append_only: bool,
) -> Result<Applied> {
    tx.batch_execute("CREATE TEMP TABLE IF NOT EXISTS import_source_stage (LIKE source_records INCLUDING DEFAULTS) ON COMMIT DROP; CREATE TEMP TABLE IF NOT EXISTS import_merchant_stage (LIKE merchants) ON COMMIT DROP; CREATE TEMP TABLE IF NOT EXISTS import_input_stage ON COMMIT DROP AS SELECT source,external_id,input_id,input_name,input_website,input_logo_url,input_logo_source,input_markets,input_aliases,input_sources,country_hints,dataset_region,negative_aliases FROM source_records WITH NO DATA")?;
    let previous: HashMap<_, _> = baseline
        .sources
        .iter()
        .map(|(id, r)| ((r.source.as_str(), r.external_id.as_str()), (id, r)))
        .collect();
    let known: HashSet<_> = baseline.merchants.iter().map(|m| m.id.as_str()).collect();
    let canonical: HashMap<_, _> = baseline
        .merchants
        .iter()
        .map(|m| (m.id.as_str(), m))
        .collect();
    let mut prepared_merchants = insert_new_merchants(tx, records, identities, &known)?;
    if append_only && !known.is_empty() {
        if let Some(ordered) = ordered_foursquare_append(tx, records, &canonical)? {
            let mut appended = HashMap::new();
            let mut countries = MarketCountries::default();
            for index in ordered.order {
                let record = &records[index];
                let id = &identities[index];
                if !ordered.eligible.contains(id) {
                    continue;
                }
                let Some(before) = canonical.get(id.as_str()) else {
                    continue;
                };
                let merchant = appended.entry(id.clone()).or_insert_with(|| {
                    countries.declaration(id, before);
                    (*before).clone()
                });
                combine_source(merchant, &record.merchant, &record.url);
                let hints: Vec<_> = record.raw["countryHints"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c.as_str().map(str::to_owned))
                    .collect();
                countries.source_fields(id, &record.merchant, &hints, None);
            }
            eprintln!(
                "Import: incrementally combining ordered Foursquare evidence for {} established merchants",
                appended.len()
            );
            prepared_merchants.extend(appended.into_iter().map(|(id, mut merchant)| {
                finish_merchant(&mut merchant, &id);
                (id, countries.hydrate(merchant))
            }));
        }
        {
            let mut reusable = known.clone();
            reusable.retain(|id| !prepared_merchants.contains_key(*id));
            for (record, id) in records.iter().zip(identities) {
                if let Some(merchant) = canonical.get(id.as_str())
                    && !redundant_place_evidence(merchant, record)
                {
                    reusable.remove(id.as_str());
                }
            }
            if !reusable.is_empty() {
                eprintln!(
                    "Import: reusing canonical evidence for {} established merchants",
                    reusable.len()
                );
            }
            prepared_merchants.extend(
                reusable
                    .into_iter()
                    .map(|id| (id.to_owned(), canonical[id].clone())),
            );
        }
    }
    let mut changed = HashSet::new();
    let mut delta = super::super::ImportDelta::default();
    let mut progress =
        crate::import_progress::Progress::new("checking/writing source records", records.len());
    if append_only {
        // The caller proved these source keys absent under the catalog write
        // lock. COPY checks uniqueness/FKs without an upsert preflight or a
        // second pair of staging tables and their join.
        copy_new_sources(tx, records, identities)?;
        delta.added = records.len();
        changed.extend(identities.iter().cloned());
        progress.advance(records.len());
    } else {
        for (batch, chunk) in records.chunks(BATCH).enumerate() {
            let mut rows = Vec::new();
            let mut updates = Vec::new();
            for (offset, r) in chunk.iter().enumerate() {
                let old = previous.get(&(r.source.as_str(), r.external_id.as_str()));
                if let Some((_, old)) = old {
                    let mut a = serde_json::to_value(old)?;
                    let mut b = serde_json::to_value(r)?;
                    b["raw"] = r.matching_raw();
                    a.as_object_mut().unwrap().remove("version");
                    b.as_object_mut().unwrap().remove("version");
                    if crate::markets::dataset_region(old) == crate::markets::dataset_region(r)
                        && a == b
                    {
                        delta.unchanged += 1;
                        continue;
                    }
                    delta.updated += 1;
                } else {
                    delta.added += 1;
                }
                // Existing source mappings are retired later by merge_groups.
                let id = old
                    .map(|(id, _)| (*id).clone())
                    .unwrap_or_else(|| identities[batch * BATCH + offset].clone());
                rows.push(vec![
                    Some(r.source.clone()),
                    Some(r.external_id.clone()),
                    Some(id.clone()),
                    r.version.clone(),
                    Some(r.attribution.clone()),
                    Some(r.license.clone()),
                    Some(r.url.clone()),
                    r.raw["transaction_text_regexp"].as_str().map(str::to_owned),
                    r.raw["parent_id"].as_str().map(str::to_owned),
                    Some(r.merchant.id.clone()),
                    Some(r.merchant.name.clone()),
                ]);
                updates.push(r);
                changed.insert(id);
            }
            copy_rows(
                tx,
                "COPY import_source_stage(source,external_id,merchant_id,version,attribution,license,url,transaction_pattern,parent_id,input_id,input_name) FROM STDIN BINARY",
                11,
                &rows,
            )?;
            copy_input_fields(tx, &updates)?;
            tx.execute("INSERT INTO source_records(source,external_id,merchant_id,version,attribution,license,url,transaction_pattern,parent_id,input_id,input_name,input_website,input_logo_url,input_logo_source,input_markets,input_aliases,input_sources,country_hints,dataset_region,negative_aliases) SELECT s.source,s.external_id,s.merchant_id,s.version,s.attribution,s.license,s.url,s.transaction_pattern,s.parent_id,i.input_id,i.input_name,i.input_website,i.input_logo_url,i.input_logo_source,i.input_markets,i.input_aliases,i.input_sources,i.country_hints,i.dataset_region,i.negative_aliases FROM import_source_stage s JOIN import_input_stage i USING(source,external_id) ON CONFLICT(source,external_id) DO UPDATE SET merchant_id=excluded.merchant_id,version=excluded.version,attribution=excluded.attribution,license=excluded.license,url=excluded.url,transaction_pattern=excluded.transaction_pattern,parent_id=excluded.parent_id,input_id=excluded.input_id,input_name=excluded.input_name,input_website=excluded.input_website,input_logo_url=excluded.input_logo_url,input_logo_source=excluded.input_logo_source,input_markets=excluded.input_markets,input_aliases=excluded.input_aliases,input_sources=excluded.input_sources,country_hints=excluded.country_hints,dataset_region=excluded.dataset_region,negative_aliases=excluded.negative_aliases",&[])?;
            tx.batch_execute(
                "TRUNCATE import_source_stage,import_merchant_stage,import_input_stage",
            )?;
            progress.advance(((batch + 1) * BATCH).min(records.len()));
        }
    }
    progress.finish();
    let mut changed: Vec<_> = changed.into_iter().collect();
    changed.sort();
    rebuild(tx, &changed, &known, &canonical, prepared_merchants)?;
    Ok(Applied {
        delta,
        refreshed_keys: changed.into_iter().collect(),
    })
}

struct OrderedAppend {
    order: Vec<usize>,
    eligible: HashSet<String>,
}

// Fence the merchant lookup before filtering source/order. Otherwise PostgreSQL
// can BitmapAnd the merchant index with the entire imported source-key prefix
// once per merchant. On Canada this read ~426k index entries 500 times per batch.
// OFFSET 0 keeps the probe restricted to this merchant; LIMIT still stops at
// the first supporting row without reading the rest of a chain's history.
const ORDERED_APPEND_PROOF: &str = r#"
SELECT e.id FROM unnest($1::text[],$2::bool[],$3::bool[]) e(id,needs_website,needs_logo)
CROSS JOIN LATERAL (
    SELECT 1 FROM (
        SELECT source,external_id,input_website,input_logo_url
        FROM source_records WHERE merchant_id=e.id OFFSET 0
    ) s
    WHERE s.source='foursquare' AND s.external_id<$4
      AND (NOT e.needs_website OR s.input_website IS NOT NULL)
      AND (NOT e.needs_logo OR s.input_logo_url IS NOT NULL)
    LIMIT 1
) prior
WHERE NOT EXISTS (SELECT 1 FROM manual_merchants m WHERE m.id=e.id)
"#;

/// Prove that each reused merchant has retained evidence preceding this batch,
/// including a website/logo when its canonical fields contain them. This keeps
/// first-source precedence even if unrelated later IDs already exist. Limit each
/// index probe to one supporting row rather than reading the merchant's history.
fn ordered_foursquare_append(
    tx: &mut Transaction<'_>,
    records: &[SourceRecord],
    canonical: &HashMap<&str, &Merchant>,
) -> Result<Option<OrderedAppend>> {
    if records.is_empty()
        || records.iter().any(|r| {
            r.source != "foursquare"
                || (r.merchant.logo_url.is_none() && r.merchant.logo_source.is_some())
        })
    {
        return Ok(None);
    }
    let progress = crate::import_progress::Progress::new(
        "checking ordered Foursquare evidence",
        canonical.len(),
    );
    let external: Vec<_> = records.iter().map(|r| r.external_id.clone()).collect();
    let rows = tx.query(
        r#"
WITH incoming AS MATERIALIZED (
    SELECT external_id,ordinality FROM unnest($1::text[]) WITH ORDINALITY i(external_id,ordinality)
)
SELECT ordinality FROM incoming
ORDER BY external_id
"#,
        &[&external],
    )?;
    if rows.len() != records.len() {
        return Ok(None);
    }
    let order: Vec<usize> = rows
        .iter()
        .map(|row| usize::try_from(row.get::<_, i64>(0) - 1).map_err(Into::into))
        .collect::<Result<_>>()?;
    let before: Vec<_> = canonical
        .values()
        .filter(|m| m.logo_url.is_some() || m.logo_source.is_none())
        .collect();
    let ids: Vec<_> = before.iter().map(|m| m.id.clone()).collect();
    let websites: Vec<_> = before.iter().map(|m| m.website.is_some()).collect();
    let logos: Vec<_> = before.iter().map(|m| m.logo_url.is_some()).collect();
    let minimum = &external[order[0]];
    let eligible: HashSet<String> = tx
        .query(ORDERED_APPEND_PROOF, &[&ids, &websites, &logos, &minimum])?
        .iter()
        .map(|row| row.get(0))
        .collect();
    progress.finish();
    Ok((!eligible.is_empty()).then_some(OrderedAppend { order, eligible }))
}

fn same_canonical_fields(a: &Merchant, b: &Merchant) -> bool {
    a.name == b.name
        && a.website == b.website
        && a.logo_url == b.logo_url
        && a.logo_source == b.logo_source
        && a.markets == b.markets
        && a.aliases == b.aliases
        && a.sources == b.sources
}

/// Adding identical positive evidence cannot change the canonical result,
/// regardless of where this new source sorts among retained sources or manual
/// fields. Reject anything that could change first-source precedence or expand
/// aliases, provenance or coverage. This is never used for source replacement.
fn redundant_place_evidence(merchant: &Merchant, record: &SourceRecord) -> bool {
    let input = &record.merchant;
    let covered = |country: &str| {
        !crate::markets::valid_country(country) || merchant.markets.iter().any(|c| c == country)
    };
    record.source == "foursquare"
        && input.name == merchant.name
        && merchant.aliases.contains(&input.name)
        && (input.website.is_none() || input.website == merchant.website)
        && ((input.logo_url == merchant.logo_url && input.logo_source == merchant.logo_source)
            || (input.logo_url.is_none() && merchant.logo_url.is_some()))
        && input.aliases.iter().all(|a| merchant.aliases.contains(a))
        && input.sources.iter().all(|s| merchant.sources.contains(s))
        && (record.url.is_empty() || merchant.sources.contains(&record.url))
        && input.markets.iter().all(|c| covered(c))
        && record.raw["countryHints"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c.as_str())
            .all(covered)
        && record.raw["places"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p["country"].as_str())
            .all(|c| covered(&c.trim().to_ascii_uppercase()))
}

fn combine_source(merchant: &mut Merchant, input: &Merchant, url: &str) {
    crate::dedupe::combine(merchant, input);
    if !url.is_empty() {
        merchant.sources.push(url.into());
    }
}
fn finish_merchant(merchant: &mut Merchant, id: &str) {
    merchant.id = id.into();
    merchant.aliases.sort();
    merchant.aliases.dedup();
    merchant.sources.sort();
    merchant.sources.dedup();
}

/// Source foreign keys require the merchant before source writes. Compute the
/// complete new identity across all incoming batches, then insert it once.
/// Sort record offsets, retaining only the new canonical merchants for rebuilding
/// their indexes without rereading source rows. Memory is bounded by the input chunk.
fn insert_new_merchants(
    tx: &mut Transaction<'_>,
    records: &[SourceRecord],
    identities: &[String],
    known: &HashSet<&str>,
) -> Result<HashMap<String, Merchant>> {
    let mut order: Vec<usize> = (0..records.len())
        .filter(|&i| !known.contains(identities[i].as_str()))
        .collect();
    order.sort_unstable_by(|&a, &b| {
        (&identities[a], &records[a].source, &records[a].external_id).cmp(&(
            &identities[b],
            &records[b].source,
            &records[b].external_id,
        ))
    });
    let total = order
        .iter()
        .enumerate()
        .filter(|(i, index)| *i == 0 || identities[**index] != identities[order[*i - 1]])
        .count();
    let mut progress =
        crate::import_progress::Progress::new("inserting new merchants with final fields", total);
    let mut inserted = 0;
    let mut rows = Vec::with_capacity(BATCH);
    let mut prepared = HashMap::with_capacity(total);
    let mut start = 0;
    while start < order.len() {
        let id = &identities[order[start]];
        let mut merchant = records[order[start]].merchant.clone();
        let mut countries = MarketCountries::default();
        let mut end = start;
        while end < order.len() && &identities[order[end]] == id {
            let record = &records[order[end]];
            combine_source(&mut merchant, &record.merchant, &record.url);
            let hints: Vec<_> = record.raw["countryHints"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            countries.source_fields(
                id,
                &record.merchant,
                &hints,
                crate::markets::dataset_region(record).as_deref(),
            );
            end += 1;
        }
        finish_merchant(&mut merchant, id);
        let merchant = countries.hydrate(merchant);
        rows.push(merchant.clone());
        prepared.insert(id.clone(), merchant);
        if rows.len() == BATCH || end == order.len() {
            copy_merchants(tx, &rows)?;
            tx.execute(UPSERT_MERCHANTS, &[])?;
            tx.batch_execute("TRUNCATE import_merchant_stage")?;
            inserted += rows.len();
            rows.clear();
            progress.advance(inserted);
        }
        start = end;
    }
    progress.finish();
    Ok(prepared)
}

fn copy_new_sources(
    tx: &mut Transaction<'_>,
    records: &[SourceRecord],
    identities: &[String],
) -> Result<()> {
    if records.len() != identities.len() {
        bail!("source identity count does not match input");
    }
    if records.is_empty() {
        return Ok(());
    }
    let sink = tx.copy_in("COPY source_records(source,external_id,merchant_id,version,attribution,license,url,transaction_pattern,parent_id,input_id,input_name,input_website,input_logo_url,input_logo_source,input_markets,input_aliases,input_sources,country_hints,dataset_region,negative_aliases) FROM STDIN BINARY")?;
    let mut types = vec![Type::TEXT; 20];
    types[14..18].fill(Type::TEXT_ARRAY);
    types[19] = Type::TEXT_ARRAY;
    let mut writer = BinaryCopyInWriter::new(sink, &types);
    for (r, id) in records.iter().zip(identities) {
        let m = &r.merchant;
        let strings = |key: &str| -> Vec<String> {
            r.raw[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        };
        writer.write(&[
            &r.source,
            &r.external_id,
            id,
            &r.version,
            &r.attribution,
            &r.license,
            &r.url,
            &r.raw["transaction_text_regexp"].as_str(),
            &r.raw["parent_id"].as_str(),
            &m.id,
            &m.name,
            &m.website,
            &m.logo_url,
            &m.logo_source,
            &m.markets,
            &m.aliases,
            &m.sources,
            &strings("countryHints"),
            &crate::markets::dataset_region(r),
            &strings("negativeAliases"),
        ])?;
    }
    writer.finish()?;
    Ok(())
}

fn copy_input_fields(tx: &mut Transaction<'_>, records: &[&SourceRecord]) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let sink = tx.copy_in("COPY import_input_stage FROM STDIN BINARY")?;
    let types = [
        Type::TEXT,
        Type::TEXT,
        Type::TEXT,
        Type::TEXT,
        Type::TEXT,
        Type::TEXT,
        Type::TEXT,
        Type::TEXT_ARRAY,
        Type::TEXT_ARRAY,
        Type::TEXT_ARRAY,
        Type::TEXT_ARRAY,
        Type::TEXT,
        Type::TEXT_ARRAY,
    ];
    let mut writer = BinaryCopyInWriter::new(sink, &types);
    for r in records {
        let m = &r.merchant;
        let hints: Vec<String> = r.raw["countryHints"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        let region = crate::markets::dataset_region(r);
        let negative_aliases: Vec<String> = r.raw["negativeAliases"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        writer.write(&[
            &r.source,
            &r.external_id,
            &m.id,
            &m.name,
            &m.website,
            &m.logo_url,
            &m.logo_source,
            &m.markets,
            &m.aliases,
            &m.sources,
            &hints,
            &region,
            &negative_aliases,
        ])?;
    }
    writer.finish()?;
    Ok(())
}

fn rebuild(
    tx: &mut Transaction<'_>,
    changed: &[String],
    known: &HashSet<&str>,
    canonical_before: &HashMap<&str, &Merchant>,
    mut prepared_merchants: HashMap<String, Merchant>,
) -> Result<()> {
    let mut progress = crate::import_progress::Progress::new(
        "rebuilding merchant search and markets",
        changed.len(),
    );
    for (batch, ids) in changed.chunks(BATCH).enumerate() {
        let existing_ids: Vec<_> = ids
            .iter()
            .filter(|id| known.contains(id.as_str()) && !prepared_merchants.contains_key(*id))
            .cloned()
            .collect();
        let reading =
            crate::import_progress::Progress::new("reading merchant source inputs", ids.len());
        let mut merchants: HashMap<String, Merchant> = if existing_ids.is_empty() {
            HashMap::new()
        } else {
            tx.query(
                "SELECT * FROM manual_merchants WHERE id=ANY($1)",
                &[&existing_ids],
            )?
            .into_iter()
            .map(|r| Ok((r.get("id"), rows::merchant(&r)?)))
            .collect::<Result<_>>()?
        };
        let mut market_index = MarketCountries::default();
        for (id, merchant) in &merchants {
            market_index.declaration(id, merchant);
        }
        if !existing_ids.is_empty() {
            typed_sources::for_each(tx, &existing_ids, |input| {
                market_index.source_fields(
                    &input.merchant_id,
                    &input.merchant,
                    &input.hints,
                    input.region.as_deref(),
                );
                let merchant = merchants
                    .entry(input.merchant_id)
                    .or_insert_with(|| input.merchant.clone());
                combine_source(merchant, &input.merchant, &input.url);
                Ok(())
            })?;
        }
        reading.finish();
        let outlets = crate::import_progress::Progress::new("reading outlet countries", ids.len());
        if !existing_ids.is_empty() {
            postgres_market_outlets(tx, &mut market_index, Some(&existing_ids))?;
        }
        outlets.finish();
        let canonical = crate::import_progress::Progress::new(
            "writing canonical merchant identities",
            ids.len(),
        );
        let mut canonical_rows = Vec::new();
        let mut canonical_keys = Vec::new();
        let mut aliases = Vec::new();
        let mut search = Vec::new();
        let mut existing_indexes = Vec::new();
        for id in ids {
            let cached = prepared_merchants.contains_key(id);
            let m = if let Some(prepared) = prepared_merchants.remove(id) {
                if let Some(before) = canonical_before.get(id.as_str())
                    && !same_canonical_fields(before, &prepared)
                {
                    canonical_rows.push(prepared.clone());
                }
                prepared
            } else {
                let mut m = merchants
                    .remove(id)
                    .context("import merchant lost its source evidence")?;
                finish_merchant(&mut m, id);
                let m = market_index.hydrate(m);
                canonical_rows.push(m.clone());
                m
            };
            let unchanged_names = cached
                && canonical_before
                    .get(id.as_str())
                    .is_some_and(|before| before.name == m.name && before.aliases == m.aliases);
            if unchanged_names {
                // Append-only evidence proved these canonical fields unchanged.
                // Their aliases/search rows already represent the same fields.
                // Still refresh rule keys, which can change between imports.
                canonical_keys.push(m);
                continue;
            }
            let mut names: Vec<_> = std::iter::once(&m.name)
                .chain(m.aliases.iter())
                .map(|n| normalize(n))
                .collect();
            names.sort();
            names.dedup();
            let before_names = if cached {
                canonical_before.get(id.as_str()).map(|before| {
                    let mut old: Vec<_> = std::iter::once(&before.name)
                        .chain(before.aliases.iter())
                        .map(|name| normalize(name))
                        .collect();
                    old.sort();
                    old.dedup();
                    old
                })
            } else {
                None
            };
            canonical_keys.push(m);
            if before_names.as_ref().is_some_and(|old| old == &names) {
                continue;
            }
            // An append-only canonical update retains all old names. Submit only
            // new normalized aliases and leave existing alias index pages alone.
            let append_names = before_names
                .as_ref()
                .filter(|old| old.iter().all(|name| names.binary_search(name).is_ok()));
            if known.contains(id.as_str()) && append_names.is_none() {
                existing_indexes.push(id.clone());
            }
            for name in &names {
                if append_names.is_none_or(|old| old.binary_search(name).is_err()) {
                    aliases.push(vec![Some(id.clone()), Some(name.clone())]);
                }
            }
            search.push(vec![Some(id.clone()), Some(names.join(" \n "))]);
        }
        if !canonical_rows.is_empty() {
            copy_merchants(tx, &canonical_rows)?;
            tx.execute(UPSERT_MERCHANTS, &[])?;
        }
        indexed_import::write_keys(tx, &canonical_keys)?;
        canonical.finish();
        let indexes = crate::import_progress::Progress::new("writing merchant aliases", ids.len());
        // Neither new identities nor reused, unchanged identities need deletes.
        write_aliases(tx, &existing_indexes, &aliases)?;
        indexes.finish();
        let searching =
            crate::import_progress::Progress::new("writing merchant search index", ids.len());
        write_search(tx, &search)?;
        searching.finish();
        tx.batch_execute("TRUNCATE import_merchant_stage")?;
        progress.advance(((batch + 1) * BATCH).min(changed.len()));
    }
    progress.finish();
    Ok(())
}

fn write_aliases(
    tx: &mut Transaction<'_>,
    existing: &[String],
    aliases: &[Vec<Option<String>>],
) -> Result<()> {
    if existing.is_empty() {
        return copy_rows(
            tx,
            "COPY aliases(merchant_id,normalized) FROM STDIN BINARY",
            2,
            aliases,
        );
    }
    // Source/market changes often leave names unchanged. Keep those index rows
    // rather than deleting and reinserting every alias of the touched merchants.
    tx.batch_execute("CREATE TEMP TABLE IF NOT EXISTS import_alias_stage(merchant_id TEXT,normalized TEXT,PRIMARY KEY(merchant_id,normalized)) ON COMMIT DROP; TRUNCATE import_alias_stage")?;
    copy_rows(tx, "COPY import_alias_stage FROM STDIN BINARY", 2, aliases)?;
    tx.execute("DELETE FROM aliases a WHERE a.merchant_id=ANY($1) AND NOT EXISTS (SELECT 1 FROM import_alias_stage s WHERE s.merchant_id=a.merchant_id AND s.normalized=a.normalized)", &[&existing])?;
    tx.execute("INSERT INTO aliases(merchant_id,normalized) SELECT merchant_id,normalized FROM import_alias_stage ON CONFLICT(merchant_id,normalized) DO NOTHING", &[])?;
    tx.batch_execute("TRUNCATE import_alias_stage")?;
    Ok(())
}

fn write_search(tx: &mut Transaction<'_>, rows: &[Vec<Option<String>>]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    tx.batch_execute("CREATE TEMP TABLE IF NOT EXISTS import_search_stage(merchant_id TEXT,text TEXT) ON COMMIT DROP; TRUNCATE import_search_stage")?;
    copy_rows(
        tx,
        "COPY import_search_stage(merchant_id,text) FROM STDIN BINARY",
        2,
        rows,
    )?;
    tx.execute("INSERT INTO merchant_search(merchant_id,text) SELECT merchant_id,text FROM import_search_stage ON CONFLICT(merchant_id) DO UPDATE SET text=excluded.text WHERE merchant_search.text IS DISTINCT FROM excluded.text", &[])?;
    tx.batch_execute("TRUNCATE import_search_stage")?;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/postgres_store/import_bulk/tests.rs"]
mod tests;
