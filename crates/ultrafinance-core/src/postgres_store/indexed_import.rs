//! Reconcile bounded input chunks against persisted, indexed merchant identities.
use super::*;
use crate::dedupe::{Decision, ImportReport, Report, Snapshot};
use postgres::{binary_copy::BinaryCopyInWriter, types::Type};
#[cfg(test)]
use serde_json::json;

pub(super) const BATCH: usize = crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE;
const DETAILS: usize = 5000;

pub(super) fn backfill(tx: &mut Transaction<'_>) -> Result<()> {
    let mut after = String::new();
    loop {
        let rows = tx.query(
            "SELECT id,name,website FROM merchants WHERE id>$1 ORDER BY id LIMIT 5000",
            &[&after],
        )?;
        if rows.is_empty() {
            break;
        }
        let merchants: Vec<Merchant> = rows
            .into_iter()
            .map(|r| Merchant {
                id: r.get(0),
                name: r.get(1),
                website: r.get(2),
                markets: vec![],

                logo_url: None,
                logo_source: None,
                aliases: vec![],
                sources: vec![],
            })
            .collect();
        after = merchants.last().unwrap().id.clone();
        write_keys(tx, &merchants)?;
    }
    Ok(())
}

pub(super) fn write_keys(tx: &mut Transaction<'_>, merchants: &[Merchant]) -> Result<()> {
    tx.batch_execute("CREATE TEMP TABLE IF NOT EXISTS identity_key_stage (LIKE merchant_identity_keys) ON COMMIT DROP; TRUNCATE identity_key_stage")?;
    let sink = tx.copy_in("COPY identity_key_stage FROM STDIN BINARY")?;
    let mut writer = BinaryCopyInWriter::new(
        sink,
        &[Type::TEXT, Type::TEXT, Type::TEXT, Type::TEXT, Type::TEXT],
    );
    for m in merchants {
        let key = crate::dedupe::deterministic_key(m);
        let name = key.as_ref().map(|k| k.0.as_str());
        let rule_host = key.as_ref().map(|k| k.1.as_deref().unwrap_or(""));
        writer.write(&[
            &m.id,
            &normalize(&m.name),
            &crate::dedupe::host(&m.website),
            &name,
            &rule_host,
        ])?;
    }
    writer.finish()?;
    tx.execute("INSERT INTO merchant_identity_keys SELECT * FROM identity_key_stage ON CONFLICT(merchant_id) DO UPDATE SET normalized_name=excluded.normalized_name,website_host=excluded.website_host,rule_name=excluded.rule_name,rule_host=excluded.rule_host WHERE (merchant_identity_keys.normalized_name,merchant_identity_keys.website_host,merchant_identity_keys.rule_name,merchant_identity_keys.rule_host) IS DISTINCT FROM (excluded.normalized_name,excluded.website_host,excluded.rule_name,excluded.rule_host)", &[])?;
    tx.batch_execute("TRUNCATE identity_key_stage")?;
    Ok(())
}

fn empty_snapshot() -> Snapshot {
    Snapshot {
        merchants: vec![],
        manual: vec![],
        sources: vec![],
        locations: vec![],
        redirects: vec![],
        location_redirects: vec![],
        revision: None,
    }
}
fn merchants(tx: &mut Transaction<'_>, ids: &[String]) -> Result<Vec<Merchant>> {
    tx.query(
        "SELECT data FROM merchants_documents WHERE id=ANY($1) ORDER BY id",
        &[&ids],
    )?
    .into_iter()
    .map(|r| Ok(serde_json::from_str(r.get(0))?))
    .collect()
}

pub(super) struct Import<'a, 'b> {
    tx: &'a mut Transaction<'b>,
    report: Report,
    delta: super::super::ImportDelta,
    run_id: String,
    selected: usize,
    chunk_size: usize,
    progress: crate::import_progress::Overall,
    provider: Option<crate::dedupe::IdentityProvider>,
    cache: Option<crate::dedupe_cache::Cache>,
}
impl<'a, 'b> Import<'a, 'b> {
    pub(super) fn new(
        tx: &'a mut Transaction<'b>,
        dry_run: bool,
        total: usize,
        chunk_size: usize,
    ) -> Result<Self> {
        tx.batch_execute("CREATE TEMP TABLE import_seen(source TEXT,external_id TEXT,PRIMARY KEY(source,external_id)) ON COMMIT DROP; CREATE TEMP TABLE incoming_keys(id TEXT PRIMARY KEY,rule_name TEXT,rule_host TEXT) ON COMMIT DROP; CREATE TEMP TABLE touched_keys(rule_name TEXT,rule_host TEXT,PRIMARY KEY(rule_name,rule_host)) ON COMMIT DROP")?;
        let count: i64 = tx.query_one("SELECT count(*) FROM merchants", &[])?.get(0);
        let revision: i64 = tx
            .query_one("SELECT revision FROM catalog_revision", &[])?
            .get(0);
        Ok(Self {
            tx,
            report: Report {
                catalog_revision: Some(revision),
                dry_run,
                model: "deterministic".into(),
                threshold: 1.0,
                candidates: 0,
                errors: 0,
                merchants_scanned: usize::try_from(count)?,
                merchants: vec![],
                groups: vec![],
                decisions: vec![],
                run_id: None,
                details_truncated: false,
            },
            delta: Default::default(),
            run_id: uuid::Uuid::new_v4().to_string(),
            selected: 0,
            chunk_size,
            progress: crate::import_progress::Overall::new(total),
            provider: None,
            cache: None,
        })
    }
    pub(super) fn set_provider(&mut self, provider: Option<crate::dedupe::IdentityProvider>) {
        self.provider = provider;
    }
    fn reconcile_brands(&mut self, touched: &[String]) -> Result<()> {
        let Some(provider) = &self.provider else {
            return Ok(());
        };
        if self.cache.is_none()
            && let Some(path) = &provider.checkpoint
        {
            self.cache = Some(crate::dedupe_cache::Cache::open(path)?);
        }
        // Names, aliases and merchant-owned hosts select comparisons, never establish identity. Compare the
        // entire affected bucket so merges require every pair, not transitivity.
        self.tx.batch_execute("SET LOCAL pg_trgm.similarity_threshold='0.3'; CREATE TEMP TABLE IF NOT EXISTS import_identity_decisions(left_id TEXT,right_id TEXT,left_digest TEXT,right_digest TEXT,PRIMARY KEY(left_id,right_id,left_digest,right_digest)) ON COMMIT DROP")?;
        use postgres::fallible_iterator::FallibleIterator;
        let mut rows = Vec::new();
        let mut seen = std::collections::HashSet::new();
        {
            let mut candidates = self.tx.query_raw(r#"
WITH names AS (SELECT DISTINCT normalized_name FROM merchant_identity_keys WHERE merchant_id=ANY($1)),
hosts AS (SELECT DISTINCT rule_host FROM merchant_identity_keys WHERE merchant_id=ANY($1) AND rule_host<>''),
labels AS (SELECT DISTINCT normalized FROM aliases WHERE merchant_id=ANY($1) AND length(normalized)>=3),
pairs AS MATERIALIZED (
SELECT a.merchant_id l,b.merchant_id r,TRUE exact FROM names n JOIN merchant_identity_keys a USING(normalized_name) JOIN merchant_identity_keys b USING(normalized_name) WHERE a.merchant_id<b.merchant_id
UNION
SELECT a.merchant_id,b.merchant_id,FALSE FROM hosts h JOIN merchant_identity_keys a USING(rule_host) JOIN merchant_identity_keys b ON b.rule_host=a.rule_host AND (b.normalized_name % a.normalized_name OR (split_part(a.normalized_name,' ',2)<>'' AND split_part(a.normalized_name,' ',1)=split_part(b.normalized_name,' ',1) AND split_part(a.normalized_name,' ',2)=split_part(b.normalized_name,' ',2)) OR strpos(' '||b.normalized_name||' ',' '||a.normalized_name||' ')>0 OR strpos(' '||a.normalized_name||' ',' '||b.normalized_name||' ')>0) WHERE a.merchant_id<b.merchant_id AND a.rule_host<>'' AND b.rule_host<>''
UNION
SELECT a.merchant_id,b.merchant_id,TRUE FROM labels l JOIN aliases a USING(normalized) JOIN aliases b USING(normalized) WHERE a.merchant_id<b.merchant_id),
ids AS (SELECT l id FROM pairs UNION SELECT r FROM pairs),
documents AS MATERIALIZED (
SELECT m.id,k.normalized_name,md5(m.data ||
    COALESCE((SELECT jsonb_agg(jsonb_build_array(s.source,s.external_id,s.input_name,s.input_website,s.input_markets,s.input_aliases) ORDER BY s.source,s.external_id)::text FROM source_records s WHERE s.merchant_id=m.id),'[]') ||
    COALESCE((SELECT data FROM manual_merchants_documents WHERE id=m.id),'')) digest
FROM ids
JOIN LATERAL (SELECT id,data FROM merchants_documents WHERE id=ids.id OFFSET 0) m ON TRUE
JOIN LATERAL (SELECT normalized_name FROM merchant_identity_keys WHERE merchant_id=ids.id OFFSET 0) k ON TRUE)

SELECT p.l,p.r,a.digest,b.digest,p.exact,a.normalized_name,b.normalized_name FROM pairs p JOIN documents a ON a.id=p.l JOIN documents b ON b.id=p.r
WHERE NOT EXISTS(SELECT 1 FROM import_identity_decisions d WHERE d.left_id=p.l AND d.right_id=p.r AND d.left_digest=a.digest AND d.right_digest=b.digest)
"#, std::iter::once(&touched as &(dyn postgres::types::ToSql + Sync)))?;
            while let Some(row) = candidates.next()? {
                let pair = (row.get::<_, String>(0), row.get::<_, String>(1));
                if seen.contains(&pair) {
                    continue;
                }
                if !row.get::<_, bool>(4)
                    && !crate::dedupe::same_host_name_candidates(
                        &row.get::<_, String>(5),
                        &row.get::<_, String>(6),
                    )
                {
                    continue;
                }
                seen.insert(pair);
                rows.push(row);
                if rows.len() > provider.max_pairs {
                    bail!(
                        "import brand reconciliation exceeds {} distinct identity pairs; increase --dedupe-max-pairs to cover the complete affected buckets or reduce --chunk-size; no changes committed",
                        provider.max_pairs
                    );
                }
            }
        }
        if rows.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = rows
            .iter()
            .flat_map(|r| [r.get::<_, String>(0), r.get::<_, String>(1)])
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let snapshot = postgres_identity_snapshot_for_ids(self.tx, &ids)?;
        let indices: HashMap<_, _> = snapshot
            .merchants
            .iter()
            .enumerate()
            .map(|(i, m)| (m.id.clone(), i))
            .collect();
        let pairs = rows
            .iter()
            .map(|r| {
                (
                    indices[&r.get::<_, String>(0)],
                    indices[&r.get::<_, String>(1)],
                )
            })
            .collect();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let evaluated = runtime.block_on(crate::dedupe::evaluate(
            &snapshot,
            pairs,
            Some(provider.api_key.clone()),
            provider.model.clone(),
            provider.threshold,
            self.report.dry_run,
            provider.max_pairs,
            &provider.endpoint,
            self.cache.as_mut(),
        ))?;
        if evaluated.errors > 0 {
            bail!(
                "{} invalid import identity answers; no changes committed",
                evaluated.errors
            );
        }
        for row in rows {
            self.tx.execute(
                "INSERT INTO import_identity_decisions VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING",
                &[
                    &row.get::<_, String>(0),
                    &row.get::<_, String>(1),
                    &row.get::<_, String>(2),
                    &row.get::<_, String>(3),
                ],
            )?;
        }
        merge_groups(self.tx, &evaluated.groups)?;
        self.report.model = provider.model.clone();
        self.report.threshold = provider.threshold;
        self.report.candidates += evaluated.candidates;
        self.report.groups.extend(evaluated.groups);
        if self.report.decisions.len() + evaluated.decisions.len() <= DETAILS
            && self.report.merchants.len() + evaluated.merchants.len() <= DETAILS
        {
            self.report.decisions.extend(evaluated.decisions);
            self.report.merchants.extend(evaluated.merchants);
        } else {
            self.report.details_truncated = true;
        }
        Ok(())
    }
    fn details(&mut self, group: Vec<String>, members: Vec<Merchant>) -> Result<()> {
        if group.len() < 2 {
            return Ok(());
        }
        let representative = members
            .iter()
            .find(|m| m.id == group[0])
            .context("merge survivor has no identity")?;
        let answer = crate::dedupe::rule_answer(representative);
        let decisions: Vec<_> = group[1..]
            .iter()
            .map(|other| Decision {
                left: group[0].clone(),
                right: other.clone(),
                answer: answer.clone(),
                accepted: true,
                method: "rule".into(),
                error: None,
            })
            .collect();
        self.report.candidates += decisions.len();
        // Keep every merge event in order even when diagnostic details are bounded.
        self.report.groups.push(group);
        if self.report.decisions.len() + decisions.len() <= DETAILS
            && self.report.merchants.len() + members.len() <= DETAILS
        {
            self.report.decisions.extend(decisions);
            for m in members {
                if !self.report.merchants.iter().any(|other| other.id == m.id) {
                    self.report.merchants.push(m);
                }
            }
        } else {
            self.report.details_truncated = true;
        }
        Ok(())
    }
    pub(super) fn chunk(&mut self, records: Vec<SourceRecord>) -> Result<()> {
        if records.len() > self.chunk_size {
            bail!("import chunk exceeds {} records", self.chunk_size);
        }
        self.tx
            .batch_execute("TRUNCATE incoming_keys,touched_keys")?;
        for r in &records {
            validate(&r.merchant)?;
            if r.source.trim().is_empty() || r.external_id.trim().is_empty() {
                bail!("source records must have unique nonblank source/external ID pairs");
            }
        }
        let matching = crate::import_progress::Progress::new(
            "looking up existing source identities",
            records.len(),
        );
        let sources: Vec<_> = records.iter().map(|r| r.source.clone()).collect();
        let external: Vec<_> = records.iter().map(|r| r.external_id.clone()).collect();
        self.tx
            .execute(
                "INSERT INTO import_seen SELECT * FROM unnest($1::text[],$2::text[])",
                &[&sources, &external],
            )
            .context("source records must have unique nonblank source/external ID pairs")?;
        let mut baseline = empty_snapshot();
        baseline.sources=self.tx.query("SELECT s.merchant_id,s.data FROM unnest($1::text[],$2::text[]) AS i(source,external_id) JOIN source_records_documents s USING(source,external_id) ORDER BY s.source,s.external_id", &[&sources,&external])?.into_iter().map(|r|Ok((r.get(0),serde_json::from_str(r.get(1))?))).collect::<Result<_>>()?;
        let previous: HashMap<_, _> = baseline
            .sources
            .iter()
            .map(|(id, r)| ((r.source.clone(), r.external_id.clone()), id.clone()))
            .collect();
        let mut existing = vec![];
        let mut existing_ids = vec![];
        let mut fresh = vec![];
        for r in records {
            if let Some(id) = previous.get(&(r.source.clone(), r.external_id.clone())) {
                existing_ids.push(id.clone());
                existing.push(r);
            } else {
                fresh.push(r);
            }
        }
        matching.finish();
        // Persist refreshed outlet countries before rebuilding stored markets,
        // so obsolete countries from this source do not survive the refresh.
        foursquare_locations::import(self.tx, &existing)?;
        baseline.merchants = merchants(self.tx, &existing_ids)?;
        // Refresh persisted source inputs first. Canonical identity includes manual
        // corrections and all retained sources, rather than only this batch.
        if !existing.is_empty() {
            let delta = import_bulk::import(self.tx, &existing, &existing_ids, &baseline)?;
            self.add_delta(delta);
        }
        let matching =
            crate::import_progress::Progress::new("matching new merchant identities", fresh.len());
        let provisional: Vec<String> = fresh
            .iter()
            .map(|_| format!("mer_{}", uuid::Uuid::new_v4().simple()))
            .collect();
        let sink = self.tx.copy_in("COPY incoming_keys FROM STDIN BINARY")?;
        let mut writer = BinaryCopyInWriter::new(sink, &[Type::TEXT, Type::TEXT, Type::TEXT]);
        for (r, id) in fresh.iter().zip(&provisional) {
            let key = crate::dedupe::deterministic_key(&r.merchant);
            let name = key.as_ref().map(|k| k.0.as_str());
            let host = key.as_ref().map(|k| k.1.as_deref().unwrap_or(""));
            writer.write(&[id, &name, &host])?;
        }
        writer.finish()?;
        // One indexed lookup per distinct incoming identity. Multiple manual
        // identities veto the entire bucket, including duplicates within a batch.
        let rows=self.tx.query(r#"
WITH keys AS (SELECT DISTINCT rule_name,rule_host FROM incoming_keys WHERE rule_name IS NOT NULL),
anchors AS (SELECT k.*, (SELECT count(*) FROM (SELECT 1 FROM merchant_identity_keys x JOIN manual_merchants m ON m.id=x.merchant_id WHERE x.rule_name=k.rule_name AND x.rule_host=k.rule_host LIMIT 2) conflict) AS manuals,
COALESCE((SELECT x.merchant_id FROM manual_merchants m JOIN merchant_identity_keys x ON m.id=x.merchant_id WHERE x.rule_name=k.rule_name AND x.rule_host=k.rule_host LIMIT 1),
(SELECT x.merchant_id FROM merchant_identity_keys x WHERE x.rule_name=k.rule_name AND x.rule_host=k.rule_host ORDER BY x.merchant_id LIMIT 1)) AS target FROM keys k)
SELECT i.id,CASE WHEN i.rule_name IS NULL OR a.manuals>1 THEN i.id ELSE COALESCE(a.target,min(i.id) OVER(PARTITION BY i.rule_name,i.rule_host)) END FROM incoming_keys i LEFT JOIN anchors a USING(rule_name,rule_host)
"#,&[])?;
        let targets: HashMap<String, String> =
            rows.into_iter().map(|r| (r.get(0), r.get(1))).collect();
        let ids: Vec<String> = provisional.iter().map(|id| targets[id].clone()).collect();
        matching.finish();
        let mut baseline = empty_snapshot();
        baseline.merchants = merchants(self.tx, &ids)?;
        if !fresh.is_empty() {
            let delta = import_bulk::import(self.tx, &fresh, &ids, &baseline)?;
            self.add_delta(delta);
            let mut groups: std::collections::BTreeMap<String, Vec<String>> = Default::default();
            for (id, target) in provisional.iter().zip(&ids) {
                if id != target {
                    groups
                        .entry(target.clone())
                        .or_insert_with(|| vec![target.clone()])
                        .push(id.clone());
                }
            }
            let fresh_by_id: HashMap<_, _> = provisional.iter().zip(&fresh).collect();
            let survivor_ids: Vec<_> = groups.keys().cloned().collect();
            let mut survivors: HashMap<_, _> = merchants(self.tx, &survivor_ids)?
                .into_iter()
                .map(|m| (m.id.clone(), m))
                .collect();
            for (target, group) in groups {
                let mut members = vec![
                    survivors
                        .remove(&target)
                        .context("merge survivor has no identity")?,
                ];
                for id in &group[1..] {
                    let mut m = fresh_by_id[id].merchant.clone();
                    m.id = id.clone();
                    members.push(m);
                }
                self.details(group, members)?;
            }
        }
        foursquare_locations::import(self.tx, &fresh)?;
        let touched: Vec<_> = existing_ids.into_iter().chain(ids).collect();
        // Refresh rule keys even when source inputs are unchanged: identity
        // rules can improve between imports without changing source metadata.
        let current = merchants(self.tx, &touched)?;
        write_keys(self.tx, &current)?;
        self.tx.execute("INSERT INTO touched_keys SELECT DISTINCT rule_name,rule_host FROM merchant_identity_keys WHERE merchant_id=ANY($1) AND rule_name IS NOT NULL ON CONFLICT DO NOTHING",&[&touched])?;
        // Only touched indexed buckets are scanned. Fetch retired IDs in bounded
        // pages even if a legacy chain has millions of duplicate merchants.
        let keys=self.tx.query("SELECT t.rule_name,t.rule_host FROM touched_keys t WHERE (SELECT count(*) FROM (SELECT 1 FROM merchant_identity_keys k WHERE k.rule_name=t.rule_name AND k.rule_host=t.rule_host LIMIT 2) matches)>1 AND (SELECT count(*) FROM (SELECT 1 FROM merchant_identity_keys k JOIN manual_merchants m ON m.id=k.merchant_id WHERE k.rule_name=t.rule_name AND k.rule_host=t.rule_host LIMIT 2) manuals)<2",&[])?;
        let mut merging =
            crate::import_progress::Progress::new("reconciling merchant groups", keys.len());
        for (index, k) in keys.into_iter().enumerate() {
            let name: String = k.get(0);
            let host: String = k.get(1);
            let target:String=self.tx.query_one("SELECT COALESCE((SELECT k.merchant_id FROM manual_merchants m JOIN merchant_identity_keys k ON m.id=k.merchant_id WHERE rule_name=$1 AND rule_host=$2 LIMIT 1),(SELECT merchant_id FROM merchant_identity_keys WHERE rule_name=$1 AND rule_host=$2 ORDER BY merchant_id LIMIT 1))",&[&name,&host])?.get(0);
            loop {
                let rows=self.tx.query("SELECT merchant_id FROM merchant_identity_keys WHERE rule_name=$1 AND rule_host=$2 AND merchant_id<>$3 ORDER BY merchant_id LIMIT 4999",&[&name,&host,&target])?;
                let group: Vec<String> = std::iter::once(target.clone())
                    .chain(rows.into_iter().map(|r| r.get(0)))
                    .collect();
                if group.len() < 2 {
                    break;
                }
                let members = merchants(self.tx, &group)?;
                self.details(group.clone(), members)?;
                merge_groups(self.tx, &[group])?;
            }
            merging.advance(index + 1);
        }
        merging.finish();
        self.reconcile_brands(&touched)?;
        self.report.merchants_scanned += fresh.len();
        self.selected += fresh.len() + existing.len();
        self.progress.report(self.selected);
        Ok(())
    }
    fn add_delta(&mut self, delta: super::super::ImportDelta) {
        self.delta.added += delta.added;
        self.delta.updated += delta.updated;
        self.delta.unchanged += delta.unchanged;
    }
    pub(super) fn finish(mut self) -> Result<ImportReport> {
        self.progress.report(self.selected);
        if self.report.candidates > 0 && !self.report.dry_run {
            self.report.run_id = Some(self.run_id);
        }
        if self.report.dry_run {
            self.delta = Default::default();
        }
        Ok(ImportReport {
            delta: self.delta,
            dedupe: self.report,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(i: usize) -> SourceRecord {
        serde_json::from_value(json!({"source":"stream-fixture","external_id":i.to_string(),"merchant":{"id":i.to_string(),"name":"Starbucks"},"attribution":"test","license":"test","url":"https://example.test","version":null,"raw":{}})).unwrap()
    }
    #[test]
    fn streaming_chunks_reuse_identity_and_late_failures_roll_back() -> Result<()> {
        let store = crate::store::MerchantStore::temporary()?;
        let directory =
            std::env::temp_dir().join(format!("ultra-stream-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory)?;
        let path = directory.join("knowledge.json");
        let mut records: Vec<_> = (0..(BATCH + 1)).map(record).collect();
        std::fs::write(&path, serde_json::to_vec(&records)?)?;
        let before = serde_json::to_value((
            store.list(None, 100, 0)?,
            store.source_records("stream-fixture", None, 100, 0)?,
        ))?;
        let (preview, available, selected) = store.reconcile_file(
            path.clone(),
            None,
            true,
            crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE,
        )?;
        assert_eq!((available, selected), (BATCH + 1, BATCH + 1));
        assert_eq!(preview.dedupe.candidates, BATCH);
        assert!(preview.dedupe.details_truncated);
        assert_eq!(
            serde_json::to_value((
                store.list(None, 100, 0)?,
                store.source_records("stream-fixture", None, 100, 0)?
            ))?,
            before
        );
        assert_eq!(store.stats()?.total, 0);
        // A duplicate source key after the first completed chunk must roll back it.
        records[BATCH] = records[0].clone();
        std::fs::write(&path, serde_json::to_vec(&records)?)?;
        assert!(
            store
                .reconcile_file(
                    path.clone(),
                    None,
                    false,
                    crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE
                )
                .is_err()
        );
        assert_eq!(
            serde_json::to_value((
                store.list(None, 100, 0)?,
                store.source_records("stream-fixture", None, 100, 0)?
            ))?,
            before
        );
        records[BATCH] = record(BATCH);
        let encoded = serde_json::to_string(&records)?;
        std::fs::write(&path, format!("{encoded} trailing garbage"))?;
        assert!(
            store
                .reconcile_file(
                    path.clone(),
                    None,
                    false,
                    crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE
                )
                .is_err()
        );
        assert_eq!(
            serde_json::to_value((
                store.list(None, 100, 0)?,
                store.source_records("stream-fixture", None, 100, 0)?
            ))?,
            before
        );
        std::fs::write(&path, encoded)?;
        let (result, _, _) = store.reconcile_file(
            path.clone(),
            None,
            false,
            crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE,
        )?;
        assert_eq!(result.delta.added, BATCH + 1);
        assert_eq!(result.dedupe.candidates, BATCH);
        assert_eq!(store.stats()?.total, 1);
        let target = store.resolve_source("stream-fixture", "0")?.unwrap();
        assert_eq!(
            store.resolve_source("stream-fixture", &BATCH.to_string())?,
            Some(target)
        );
        let (refresh, _, selected) = store.reconcile_file(
            path,
            Some(2),
            false,
            crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE,
        )?;
        assert_eq!(selected, 2);
        assert_eq!(refresh.delta.unchanged, 2);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }
    #[test]
    fn configured_chunks_preserve_dedupe_and_rollback() -> Result<()> {
        for size in [1, 2, 17, 10000] {
            let store = crate::store::MerchantStore::temporary()?;
            let records: Vec<_> = (0..19).map(record).collect();
            assert!(store.reconcile_import(records.clone(), false, 0).is_err());
            let preview = store.reconcile_import(records.clone(), true, size)?;
            assert_eq!(preview.dedupe.candidates, 18);
            assert_eq!(store.stats()?.total, 0);
            let result = store.reconcile_import(records.clone(), false, size)?;
            assert_eq!(result.delta.added, 19);
            assert_eq!(result.dedupe.candidates, 18);
            assert_eq!(store.stats()?.total, 1);
            let before = serde_json::to_value((
                store.list(None, 100, 0)?,
                store.source_records("stream-fixture", None, 100, 0)?,
            ))?;
            let mut invalid = records.clone();
            invalid.push(records[0].clone());
            assert!(store.reconcile_import(invalid, false, size).is_err());
            assert_eq!(
                serde_json::to_value((
                    store.list(None, 100, 0)?,
                    store.source_records("stream-fixture", None, 100, 0)?
                ))?,
                before
            );
            assert_eq!(
                store
                    .reconcile_import(records, false, size)?
                    .delta
                    .unchanged,
                19
            );
        }
        Ok(())
    }
    #[tokio::test]
    async fn concurrent_imports_share_indexed_identity() -> Result<()> {
        let store = crate::store::MerchantStore::temporary()?;
        let a = crate::dedupe::import(store.clone(), vec![record(0)], Default::default());
        let b = crate::dedupe::import(store.clone(), vec![record(1)], Default::default());
        let (a, b) = tokio::join!(a, b);
        a?;
        b?;
        assert_eq!(store.stats()?.total, 1);
        assert_eq!(
            store.resolve_source("stream-fixture", "0")?,
            store.resolve_source("stream-fixture", "1")?
        );
        Ok(())
    }
    #[test]
    fn version_six_identity_backfill_crosses_bounded_pages() -> Result<()> {
        let lease = crate::store::MerchantStore::temporary()?;
        let base = lease.temporary_url().to_owned();
        let legacy = PostgresStore::connect(&base, false)?;
        legacy.run(|client| {
            client.batch_execute("INSERT INTO merchants(id,name,website,markets_json,aliases_json,sources_json) SELECT 'legacy-'||lpad(i::text,8,'0'),'Café Brand LLC',CASE WHEN i%2=0 THEN 'https://www.brand.test/ca' ELSE NULL END,'[]','[]','[]' FROM generate_series(0,5000) i; DROP TABLE merchant_identity_keys; DROP FUNCTION advance_catalog_revision() CASCADE; DROP TABLE catalog_revision; UPDATE ultrafinance_schema SET version=6")?;
            Ok(())
        })?;
        let upgraded = PostgresStore::connect(&base, true)?;
        upgraded.run(|client| {
            let count:i64=client.query_one("SELECT count(*) FROM merchant_identity_keys",&[])?.get(0);
            assert_eq!(count,5001);
            let row=client.query_one("SELECT rule_name,rule_host FROM merchant_identity_keys WHERE merchant_id='legacy-00005000'",&[])?;
            assert_eq!(row.get::<_,String>(0),"cafe brand");assert_eq!(row.get::<_,String>(1),"brand.test");
            let row=client.query_one("SELECT rule_name,rule_host FROM merchant_identity_keys WHERE merchant_id='legacy-00004999'",&[])?;
            assert_eq!(row.get::<_,String>(0),"cafe brand llc");assert_eq!(row.get::<_,String>(1),"");
            assert_eq!(client.query_one("SELECT version FROM ultrafinance_schema",&[])?.get::<_,i32>(0),12);
            Ok(())
        })
    }

    #[test]
    fn identity_index_tracks_corrections_and_uses_indexed_lookup() -> Result<()> {
        let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
            .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
        let store = PostgresStore::temporary(&base)?;
        let mut m = record(0).merchant;
        m.id = "manual".into();
        m.name = "Café Brand, LLC".into();
        m.website = Some("https://www.brand.test/ca".into());
        store.put(&m)?;
        store.run(|client| {
            client.batch_execute("SET enable_seqscan=off")?;
            let row=client.query_one("SELECT rule_name,rule_host FROM merchant_identity_keys WHERE merchant_id='manual'",&[])?;
            assert_eq!(row.get::<_,String>(0),"cafe brand");assert_eq!(row.get::<_,String>(1),"brand.test");
            let plan=client.query("EXPLAIN SELECT merchant_id FROM merchant_identity_keys WHERE rule_name='cafe brand' AND rule_host='brand.test'",&[])?;
            assert!(plan.iter().any(|r|r.get::<_,String>(0).contains("merchant_identity_rule")));
            Ok(())
        })?;
        m.name = "Renamed".into();
        m.website = None;
        store.put(&m)?;
        store.run(|client| {
            let row = client.query_one(
                "SELECT rule_name,rule_host FROM merchant_identity_keys WHERE merchant_id='manual'",
                &[],
            )?;
            assert_eq!(row.get::<_, String>(0), "renamed");
            assert_eq!(row.get::<_, String>(1), "");
            Ok(())
        })
    }
}
