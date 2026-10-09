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
fn merchant_row(m: &Merchant) -> Result<Vec<Option<String>>> {
    Ok(vec![
        Some(m.id.clone()),
        Some(m.name.clone()),
        m.website.clone(),
        m.logo_url.clone(),
        m.logo_source.clone(),
        Some(serde_json::to_string(&m.markets)?),
        Some(serde_json::to_string(&m.aliases)?),
        Some(serde_json::to_string(&m.sources)?),
    ])
}
const COPY_MERCHANTS: &str = "COPY import_merchant_stage(id,name,website,logo_url,logo_source,markets_json,aliases_json,sources_json) FROM STDIN BINARY";
const UPSERT_MERCHANTS: &str = "INSERT INTO merchants(id,name,website,logo_url,logo_source,markets_json,aliases_json,sources_json) SELECT id,name,website,logo_url,logo_source,markets_json,aliases_json,sources_json FROM import_merchant_stage ON CONFLICT(id) DO UPDATE SET name=excluded.name,website=excluded.website,logo_url=excluded.logo_url,logo_source=excluded.logo_source,markets_json=excluded.markets_json,aliases_json=excluded.aliases_json,sources_json=excluded.sources_json WHERE (merchants.name,merchants.website,merchants.logo_url,merchants.logo_source,merchants.markets_json,merchants.aliases_json,merchants.sources_json) IS DISTINCT FROM (excluded.name,excluded.website,excluded.logo_url,excluded.logo_source,excluded.markets_json,excluded.aliases_json,excluded.sources_json)";

pub(super) fn import(
    tx: &mut Transaction<'_>,
    records: &[SourceRecord],
    identities: &[String],
    baseline: &crate::dedupe::Snapshot,
) -> Result<super::super::ImportDelta> {
    tx.batch_execute("CREATE TEMP TABLE IF NOT EXISTS import_source_stage (LIKE source_records INCLUDING DEFAULTS) ON COMMIT DROP; CREATE TEMP TABLE IF NOT EXISTS import_merchant_stage (LIKE merchants) ON COMMIT DROP; CREATE TEMP TABLE IF NOT EXISTS import_input_stage ON COMMIT DROP AS SELECT source,external_id,input_id,input_name,input_website,input_logo_url,input_logo_source,input_markets,input_aliases,input_sources,country_hints,dataset_region,negative_aliases FROM source_records WITH NO DATA")?;
    let previous: HashMap<_, _> = baseline
        .sources
        .iter()
        .map(|(id, r)| ((r.source.as_str(), r.external_id.as_str()), (id, r)))
        .collect();
    let known: HashSet<_> = baseline.merchants.iter().map(|m| m.id.as_str()).collect();
    insert_new_merchants(tx, records, identities, &known)?;
    let mut changed = HashSet::new();
    let mut delta = super::super::ImportDelta::default();
    let mut progress =
        crate::import_progress::Progress::new("checking/writing source records", records.len());
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
        tx.batch_execute("TRUNCATE import_source_stage,import_merchant_stage,import_input_stage")?;
        progress.advance(((batch + 1) * BATCH).min(records.len()));
    }
    progress.finish();
    let mut changed: Vec<_> = changed.into_iter().collect();
    changed.sort();
    rebuild(tx, &changed, &known)?;
    Ok(delta)
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
/// Sorting only record offsets avoids allocating another catalog of merchants.
fn insert_new_merchants(
    tx: &mut Transaction<'_>,
    records: &[SourceRecord],
    identities: &[String],
    known: &HashSet<&str>,
) -> Result<()> {
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
        rows.push(merchant_row(&merchant)?);
        if rows.len() == BATCH || end == order.len() {
            copy_rows(tx, COPY_MERCHANTS, 8, &rows)?;
            tx.execute(UPSERT_MERCHANTS, &[])?;
            tx.batch_execute("TRUNCATE import_merchant_stage")?;
            inserted += rows.len();
            rows.clear();
            progress.advance(inserted);
        }
        start = end;
    }
    progress.finish();
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

fn rebuild(tx: &mut Transaction<'_>, changed: &[String], known: &HashSet<&str>) -> Result<()> {
    let mut progress = crate::import_progress::Progress::new(
        "rebuilding merchant search and markets",
        changed.len(),
    );
    for (batch, ids) in changed.chunks(BATCH).enumerate() {
        let reading =
            crate::import_progress::Progress::new("reading merchant source inputs", ids.len());
        let mut merchants: HashMap<String, Merchant> = tx
            .query(
                "SELECT id,data FROM manual_merchants_documents WHERE id=ANY($1)",
                &[&ids],
            )?
            .into_iter()
            .map(|r| Ok((r.get(0), serde_json::from_str(r.get::<_, &str>(1))?)))
            .collect::<Result<_>>()?;
        let mut market_index = MarketCountries::default();
        for (id, merchant) in &merchants {
            market_index.declaration(id, merchant);
        }
        typed_sources::for_each(tx, ids, |input| {
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
        reading.finish();
        let outlets = crate::import_progress::Progress::new("reading outlet countries", ids.len());
        let existing_ids: Vec<_> = ids
            .iter()
            .filter(|id| known.contains(id.as_str()))
            .cloned()
            .collect();
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
        for id in ids {
            let mut m = merchants
                .remove(id)
                .context("import merchant lost its source evidence")?;
            finish_merchant(&mut m, id);
            let m = market_index.hydrate(m);
            let mut names: Vec<_> = std::iter::once(&m.name)
                .chain(m.aliases.iter())
                .map(|n| normalize(n))
                .collect();
            names.sort();
            names.dedup();
            canonical_rows.push(merchant_row(&m)?);
            canonical_keys.push(m.clone());
            for name in &names {
                aliases.push(vec![Some(id.clone()), Some(name.clone())]);
            }
            search.push(vec![Some(id.clone()), Some(names.join(" \n "))]);
        }
        copy_rows(tx, COPY_MERCHANTS, 8, &canonical_rows)?;
        tx.execute(UPSERT_MERCHANTS, &[])?;
        indexed_import::write_keys(tx, &canonical_keys)?;
        canonical.finish();
        let indexes = crate::import_progress::Progress::new("writing merchant aliases", ids.len());
        // New identities have no old index rows to delete. Avoid scans of growing
        // tables for batches made up entirely of newly inserted merchants.
        let existing: Vec<_> = ids
            .iter()
            .filter(|id| known.contains(id.as_str()))
            .cloned()
            .collect();
        if !existing.is_empty() {
            tx.execute(
                "DELETE FROM aliases WHERE merchant_id=ANY($1)",
                &[&existing],
            )?;
        }
        copy_rows(
            tx,
            "COPY aliases(merchant_id,normalized) FROM STDIN BINARY",
            2,
            &aliases,
        )?;
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

fn write_search(tx: &mut Transaction<'_>, rows: &[Vec<Option<String>>]) -> Result<()> {
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
mod tests {
    use super::*;
    #[test]
    fn copy_roundtrips_null_unicode_empty_and_large_text() -> Result<()> {
        let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
            .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
        let store = PostgresStore::temporary(&base)?;
        store.run(|client| {
            let mut tx = client.transaction()?;
            tx.batch_execute("CREATE TEMP TABLE copy_roundtrip(a TEXT,b TEXT)")?;
            let rows = vec![
                vec![None, Some(String::new())],
                vec![
                    Some("Café 東京\n\t\\\"".into()),
                    Some("x".repeat(2 * 1024 * 1024)),
                ],
            ];
            copy_rows(
                &mut tx,
                "COPY copy_roundtrip(a,b) FROM STDIN BINARY",
                2,
                &rows,
            )?;
            let actual: Vec<Vec<Option<String>>> = tx
                .query("SELECT a,b FROM copy_roundtrip ORDER BY a NULLS FIRST", &[])?
                .iter()
                .map(|r| vec![r.get(0), r.get(1)])
                .collect();
            assert_eq!(actual, rows);
            tx.commit()?;
            Ok(())
        })
    }
    #[test]
    fn fresh_merchants_insert_final_fields_once_across_source_batches() -> Result<()> {
        let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
            .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
        let store = PostgresStore::temporary(&base)?;
        store.run(|client| {
            client.batch_execute("CREATE TABLE merchant_writes(id TEXT,operation TEXT,aliases TEXT); CREATE FUNCTION record_merchant_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO merchant_writes VALUES(NEW.id,TG_OP,NEW.aliases_json); RETURN NEW; END $$; CREATE TRIGGER merchant_write AFTER INSERT OR UPDATE ON merchants FOR EACH ROW EXECUTE FUNCTION record_merchant_write()")?;
            Ok(())
        })?;
        let records: Vec<SourceRecord> = (0..BATCH + 1)
            .map(|i| {
                SourceRecord {
            source:"single-write".into(),external_id:format!("{i:08}"),
            merchant:serde_json::from_value(serde_json::json!({
                "id":format!("input-{i}"),"name":format!("Merchant {}",if i==BATCH {0} else {i}),
                "aliases":[if i==BATCH {"BANK LAST"} else {"BANK FIRST"}],
                "markets":[if i==BATCH {"US"} else {"CA"}]
            })).unwrap(),
            attribution:"test".into(),license:"test".into(),url:format!("https://example.org/{i}"),
            version:None,raw:serde_json::json!({}),
        }
            })
            .collect();
        let ids: Vec<_> = (0..BATCH + 1)
            .map(|i| format!("canonical-{}", if i == BATCH { 0 } else { i }))
            .collect();
        let expected = store.dedupe_snapshot()?;
        let mut staged = expected.clone();
        staged.merchants = records[..BATCH]
            .iter()
            .zip(&ids)
            .map(|(r, id)| {
                let mut m = r.merchant.clone();
                m.id = id.clone();
                m
            })
            .collect();
        staged.sources = ids.iter().cloned().zip(records.iter().cloned()).collect();
        let (delta, _) = store.apply_reconciled_import(
            &expected,
            &staged,
            &records,
            &ids,
            &[],
            &serde_json::json!({}),
        )?;
        assert_eq!(delta.added, BATCH + 1);
        let first = store.get(&ids[0])?.unwrap();
        assert!(first.aliases.contains(&"BANK FIRST".into()));
        assert!(first.aliases.contains(&"BANK LAST".into()));
        assert_eq!(first.markets, vec!["CA", "US"]);
        assert_eq!(first.sources.len(), 2);
        store.run(|client| {
            assert_eq!(
                client
                    .query_one(
                        "SELECT count(*) FROM merchant_writes WHERE operation='INSERT'",
                        &[]
                    )?
                    .get::<_, i64>(0),
                BATCH as i64
            );
            assert_eq!(
                client
                    .query_one(
                        "SELECT count(*) FROM merchant_writes WHERE operation='UPDATE'",
                        &[]
                    )?
                    .get::<_, i64>(0),
                0
            );
            let inserted: String = client
                .query_one(
                    "SELECT aliases FROM merchant_writes WHERE id='canonical-0'",
                    &[],
                )?
                .get(0);
            assert!(serde_json::from_str::<Vec<String>>(&inserted)?.contains(&"BANK LAST".into()));
            Ok(())
        })?;
        // A real refresh of an established merchant must still update it.
        let mut refresh = records[BATCH].clone();
        refresh.merchant.aliases = vec!["BANK LAST CHANGED".into()];
        let expected = store.dedupe_snapshot()?;
        let (delta, _) = store.apply_reconciled_import(
            &expected,
            &expected,
            &[refresh],
            &[ids[0].clone()],
            &[],
            &serde_json::json!({}),
        )?;
        assert_eq!(delta.updated, 1);
        assert!(
            store
                .get(&ids[0])?
                .unwrap()
                .aliases
                .contains(&"BANK LAST CHANGED".into())
        );
        store.run(|client| {
            assert_eq!(
                client
                    .query_one(
                        "SELECT count(*) FROM merchant_writes WHERE operation='UPDATE'",
                        &[]
                    )?
                    .get::<_, i64>(0),
                1
            );
            Ok(())
        })?;
        Ok(())
    }

    #[test]
    fn search_index_only_changes_when_searchable_names_change() -> Result<()> {
        let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
            .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
        let store = PostgresStore::temporary(&base)?;
        let mut record: SourceRecord = serde_json::from_value(serde_json::json!({
            "source":"search-write-test", "external_id":"one",
            "merchant":{"id":"input", "name":"Alpha Café"},
            "attribution":"test", "license":"test", "url":"https://example.test", "raw":{}
        }))?;
        store.reconcile_import(vec![record.clone()], false, 5000)?;
        store.run(|client| {
            client.batch_execute("CREATE TABLE search_writes(operation TEXT); CREATE FUNCTION record_search_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO search_writes VALUES(TG_OP); RETURN NEW; END $$; CREATE TRIGGER search_write AFTER INSERT OR UPDATE OR DELETE ON merchant_search FOR EACH ROW EXECUTE FUNCTION record_search_write()")?;
            Ok(())
        })?;
        // A changed source input rebuilds coverage/identity, but not unchanged search text.
        record.merchant.website = Some("https://alpha.example".into());
        assert_eq!(
            store
                .reconcile_import(vec![record.clone()], false, 5000)?
                .delta
                .updated,
            1
        );
        store.run(|client| {
            assert_eq!(
                client
                    .query_one("SELECT count(*) FROM search_writes", &[])?
                    .get::<_, i64>(0),
                0
            );
            Ok(())
        })?;
        record.merchant.name = "Beta Café".into();
        store.reconcile_import(vec![record], false, 5000)?;
        store.run(|client| {
            assert_eq!(
                client
                    .query_one("SELECT operation FROM search_writes", &[])?
                    .get::<_, String>(0),
                "UPDATE"
            );
            assert_eq!(
                client
                    .query_one("SELECT text FROM merchant_search", &[])?
                    .get::<_, String>(0),
                "beta cafe"
            );
            assert_eq!(
                client
                    .query_one("SELECT normalized FROM aliases", &[])?
                    .get::<_, String>(0),
                "beta cafe"
            );
            Ok(())
        })?;
        Ok(())
    }
    #[test]
    fn later_search_batch_failure_rolls_back_source_copies_and_indexes() -> Result<()> {
        let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
            .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
        let store = PostgresStore::temporary(&base)?;
        store.run(|client| {
            client.batch_execute("ALTER TABLE merchant_search ADD CONSTRAINT import_test_failure CHECK (text <> 'forced failure')")?;
            Ok(())
        })?;
        let expected = store.dedupe_snapshot()?;
        let records: Vec<SourceRecord> = (0..BATCH+1).map(|i| SourceRecord {
            source: "bulk-rollback".into(),external_id:i.to_string(),
            merchant: serde_json::from_value(serde_json::json!({"id":format!("id-{i}"),"name":if i==BATCH {"Forced failure".into()} else {format!("Merchant {i}")},"markets":["CA"]})).unwrap(),
            attribution:"test".into(),license:"test".into(),url:"https://example.org".into(),version:None,raw:serde_json::json!({}),
        }).collect();
        let ids: Vec<_> = records.iter().map(|r| r.merchant.id.clone()).collect();
        let mut staged = expected.clone();
        staged.merchants = records.iter().map(|r| r.merchant.clone()).collect();
        staged.sources = ids.iter().cloned().zip(records.iter().cloned()).collect();
        let result = store.apply_reconciled_import(
            &expected,
            &staged,
            &records,
            &ids,
            &[],
            &serde_json::json!({}),
        );
        assert!(result.is_err());
        let after = store.dedupe_snapshot()?;
        assert!(after.merchants.is_empty());
        assert!(after.sources.is_empty());
        store.run(|client| {
            for table in ["aliases", "merchant_search"] {
                let count: i64 = client
                    .query_one(&format!("SELECT count(*) FROM {table}"), &[])?
                    .get(0);
                assert_eq!(count, 0);
            }
            Ok(())
        })?;
        Ok(())
    }
}
