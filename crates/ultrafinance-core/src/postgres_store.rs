//! Synchronous store on a dedicated worker: postgres owns a Tokio runtime and
//! must never be constructed or dropped on the API/CLI's async runtime threads.
use super::{
    Candidate, Merchant, MerchantPage, SourceRecord, excluded, normalize, rank_candidates, validate,
};
use super::{MerchantSourceStats, MerchantStats, STATS_SOURCES, STATS_TOTALS, location_reference};
use crate::location::LocationRecord;
use crate::markets::{MarketIndex, market_counts, paginate};
use anyhow::{Context, Result, bail};
use postgres::{Client, GenericClient, IsolationLevel, Transaction};
use postgres_native_tls::MakeTlsConnector;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, mpsc},
    time::Duration,
};

type Job = Box<dyn FnOnce(Result<&mut Client>) + Send>;
#[derive(Clone)]
pub(super) struct PostgresStore(Arc<mpsc::Sender<Job>>);
const WRITE_LOCK: i64 = 0x756c74726166696e;

impl PostgresStore {
    pub fn connect(url: &str, initialize: bool) -> Result<Self> {
        Self::connect_with_mode(url, initialize, false)
    }
    pub fn lazy(url: &str) -> Result<Self> {
        Self::connect_with_mode(url, false, true)
    }
    fn connect_with_mode(url: &str, initialize: bool, lazy: bool) -> Result<Self> {
        // Parse outside the worker, but never attach the URL to diagnostics.
        let mut config: postgres::Config = url
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid PostgreSQL connection configuration"))?;
        config.connect_timeout(Duration::from_secs(35));
        config.application_name("ultrafinance");
        let (sender, receiver) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        std::thread::Builder::new().name("merchant-postgres".into()).spawn(move || {
            let connect = || -> Result<Client> {
                let tls = MakeTlsConnector::new(native_tls::TlsConnector::builder().build()?);
                let mut client = config.connect(tls).context("PostgreSQL connection failed")?;
                // Server-side expiry continues while Lambda's worker is frozen.
                client.batch_execute("SET statement_timeout = '15s'; SET lock_timeout = '10s'; SET idle_in_transaction_session_timeout = '30s'; SET idle_session_timeout = '60s'")?;
                Ok(client)
            };
            let setup = || -> Result<Client> {
                let mut client = connect()?;
                if initialize {
                    let mut tx = client.transaction()?;
                    write_lock(&mut tx)?;
                    let exists: bool = tx.query_one("SELECT to_regclass('public.ultrafinance_schema') IS NOT NULL", &[])?.get(0);
                    if !exists { tx.batch_execute(include_str!("../migrations/001_postgres.sql"))?; }
                    let version: i32 = tx.query_one("SELECT version FROM ultrafinance_schema", &[])?.get(0);
                    if version == 1 { tx.batch_execute(include_str!("../migrations/002_enrichment_log.sql"))?; }
                    if version <= 2 { tx.batch_execute(include_str!("../migrations/003_locations.sql"))?; }
                    if version <= 2 { migrate_markets(&mut tx)?; }
                    tx.commit()?;
                }
                let version: i32 = client.query_one("SELECT version FROM ultrafinance_schema", &[])
                    .context("PostgreSQL schema missing; run `ultrafinance database init` first")?.get(0);
                if version != 3 { bail!("unsupported PostgreSQL schema version {version}; run `ultrafinance database init` to migrate markets"); }
                Ok(client)
            };
            let mut client = match if lazy { Ok(None) } else { setup().map(Some) } {
                Ok(client) => { let _ = ready_tx.send(Ok(())); client },
                Err(error) => { let _ = ready_tx.send(Err(error)); return; }
            };
            for job in receiver {
                // Probe before executing the job: frozen clients may not have
                // processed the server's idle disconnect yet. Never replay a job
                // (especially a write) after it has started.
                let healthy = client.as_mut().is_some_and(|c| !c.is_closed() && c.is_valid(Duration::from_secs(2)).is_ok());
                if !healthy {
                    client = None;
                    match setup() {
                        Ok(replacement) => client = Some(replacement),
                        Err(error) => { job(Err(error)); continue; }
                    }
                }
                job(Ok(client.as_mut().expect("connection prepared before job")));
            }
        })?;
        ready_rx
            .recv()
            .context("PostgreSQL worker stopped during startup")??;
        Ok(Self(Arc::new(sender)))
    }
    fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Client) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.0
            .send(Box::new(move |client| {
                let _ = sender.send(client.and_then(f));
            }))
            .map_err(|_| anyhow::anyhow!("PostgreSQL worker stopped"))?;
        receiver.recv().context("PostgreSQL worker stopped")?
    }
    pub fn write_log(
        &self,
        id: &str,
        batch: &str,
        status: &str,
        merchant: Option<&str>,
        data: &str,
    ) -> Result<()> {
        let (id, batch, status, merchant, data) = (
            id.to_owned(),
            batch.to_owned(),
            status.to_owned(),
            merchant.map(str::to_owned),
            data.to_owned(),
        );
        self.run(move |client| {
            client.execute("INSERT INTO enrichment_log(id,batch_id,status,merchant_id,data,finished_at) VALUES($1,$2,$3,$4,$5,CASE WHEN $3='started' THEN NULL ELSE clock_timestamp() END) ON CONFLICT(id) DO UPDATE SET status=excluded.status,merchant_id=excluded.merchant_id,data=excluded.data,finished_at=excluded.finished_at", &[&id,&batch,&status,&merchant,&data])?;
            Ok(())
        })
    }
    pub fn logs(
        &self,
        status: Option<&str>,
        merchant: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<serde_json::Value>> {
        let (status, merchant) = (status.map(str::to_owned), merchant.map(str::to_owned));
        self.run(move |client| {
            client.query("SELECT id,batch_id,status,merchant_id,created_at::text,finished_at::text,data FROM enrichment_log WHERE ($1::text IS NULL OR status=$1) AND ($2::text IS NULL OR merchant_id=$2) ORDER BY created_at DESC,id LIMIT $3 OFFSET $4", &[&status,&merchant,&(limit as i64),&(offset as i64)])?.into_iter().map(|r| {
                Ok(serde_json::json!({"id":r.get::<_,String>(0),"batch_id":r.get::<_,String>(1),"status":r.get::<_,String>(2),"merchant_id":r.get::<_,Option<String>>(3),"created_at":r.get::<_,String>(4),"finished_at":r.get::<_,Option<String>>(5),"data":serde_json::from_str::<serde_json::Value>(&r.get::<_,String>(6))?}))
            }).collect()
        })
    }
    pub fn import_locations(&self, records: &[LocationRecord]) -> Result<()> {
        let records = records.to_vec();
        self.run(move |client| {
            let mut tx = client.transaction()?;
            write_lock(&mut tx)?;
            for record in records {
                let existing: Option<String> = tx.query_opt("SELECT data FROM location_records WHERE source=$1 AND external_id=$2", &[&record.source,&record.external_id])?.map(|r| r.get(0));
                if !record.manual_override && existing.as_deref().map(serde_json::from_str::<LocationRecord>).transpose()?.is_some_and(|r| r.manual_override) { continue; }
                let (merchant_id, merchant_source, merchant_external) = location_reference(&record.merchant);
                let exists: bool = if let Some(id) = merchant_id {
                    tx.query_one("SELECT EXISTS(SELECT 1 FROM merchants WHERE id=$1)", &[&id])?.get(0)
                } else {
                    tx.query_one("SELECT EXISTS(SELECT 1 FROM source_records WHERE source=$1 AND external_id=$2)", &[&merchant_source,&merchant_external])?.get(0)
                };
                if !exists { bail!("outlet merchant reference is missing; import or link its merchant first"); }
                let id: String = tx.query_opt("SELECT id FROM location_records WHERE source=$1 AND external_id=$2", &[&record.source,&record.external_id])?.map(|r| r.get(0)).unwrap_or_else(|| format!("loc_{}", uuid::Uuid::new_v4().simple()));
                let mut stored = record.clone();
                stored.location.id = Some(id.clone());
                tx.execute("INSERT INTO location_records(id,source,external_id,merchant_id,merchant_source,merchant_external_id,data) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(source,external_id) DO UPDATE SET merchant_id=excluded.merchant_id,merchant_source=excluded.merchant_source,merchant_external_id=excluded.merchant_external_id,data=excluded.data", &[&id,&record.source,&record.external_id,&merchant_id,&merchant_source,&merchant_external,&serde_json::to_string(&stored)?])?;
            }
            tx.commit()?;
            Ok(())
        })
    }
    pub fn locations(&self, merchant_id: &str) -> Result<Vec<LocationRecord>> {
        let merchant_id = merchant_id.to_owned();
        self.run(move |client| {
            client.query("SELECT l.data FROM location_records l LEFT JOIN source_records s ON s.source=l.merchant_source AND s.external_id=l.merchant_external_id WHERE COALESCE(l.merchant_id,s.merchant_id)=$1 ORDER BY l.id", &[&merchant_id])?.iter().map(|row| Ok(serde_json::from_str(row.get::<_, &str>(0))?)).collect()
        })
    }
    pub fn put(&self, merchant: &Merchant) -> Result<()> {
        validate(merchant)?;
        let merchant = merchant.clone();
        self.run(move |client| {
            let mut tx = client.transaction()?;
            write_lock(&mut tx)?;
            tx.execute("INSERT INTO manual_merchants VALUES($1,$2) ON CONFLICT(id) DO UPDATE SET data=excluded.data", &[&merchant.id, &serde_json::to_string(&merchant)?])?;
            rebuild(&mut tx, &merchant.id)?;
            tx.commit()?;
            Ok(())
        })
    }
    pub fn import(&self, records: &[SourceRecord]) -> Result<()> {
        let mut keys = HashSet::new();
        for r in records {
            validate(&r.merchant)?;
            if r.source.trim().is_empty()
                || r.external_id.trim().is_empty()
                || !keys.insert((&r.source, &r.external_id))
            {
                bail!("source records must have unique nonblank source/external ID pairs");
            }
        }
        let records = records.to_vec();
        self.run(move |client| {
            let mut tx = client.transaction()?;
            write_lock(&mut tx)?;
            let mut changed = HashSet::new();
            for record in records {
                let id: String = tx.query_opt("SELECT merchant_id FROM source_records WHERE source=$1 AND external_id=$2", &[&record.source,&record.external_id])?
                    .map(|r| r.get(0)).unwrap_or_else(|| format!("mer_{}", uuid::Uuid::new_v4().simple()));
                tx.execute("INSERT INTO merchants(id,data) VALUES($1,$2) ON CONFLICT(id) DO NOTHING", &[&id,&serde_json::to_string(&record.merchant)?])?;
                tx.execute("INSERT INTO source_records VALUES($1,$2,$3,$4) ON CONFLICT(source,external_id) DO UPDATE SET data=excluded.data", &[&record.source,&record.external_id,&id,&serde_json::to_string(&record)?])?;
                changed.insert(id);
            }
            for id in changed { rebuild(&mut tx, &id)?; }
            tx.commit()?;
            Ok(())
        })
    }
    pub fn link(&self, source: &str, external_id: &str, target: &str) -> Result<()> {
        let (source, external_id, target) =
            (source.to_owned(), external_id.to_owned(), target.to_owned());
        self.run(move |client| {
            let mut tx = client.transaction()?;
            write_lock(&mut tx)?;
            let old: String = tx
                .query_one(
                    "SELECT merchant_id FROM source_records WHERE source=$1 AND external_id=$2",
                    &[&source, &external_id],
                )?
                .get(0);
            if tx
                .query_opt("SELECT id FROM merchants WHERE id=$1", &[&target])?
                .is_none()
            {
                bail!("target merchant does not exist");
            }
            tx.execute(
                "UPDATE source_records SET merchant_id=$3 WHERE source=$1 AND external_id=$2",
                &[&source, &external_id, &target],
            )?;
            rebuild(&mut tx, &target)?;
            if old != target {
                rebuild(&mut tx, &old)?;
            }
            tx.commit()?;
            Ok(())
        })
    }
    pub fn resolve_source(&self, source: &str, external_id: &str) -> Result<Option<String>> {
        let (source, external_id) = (source.to_owned(), external_id.to_owned());
        self.run(move |c| {
            Ok(c.query_opt(
                "SELECT merchant_id FROM source_records WHERE source=$1 AND external_id=$2",
                &[&source, &external_id],
            )?
            .map(|r| r.get(0)))
        })
    }
    pub fn fingerprint(&self) -> Result<String> {
        self.run(|client| {
            let mut tx = client
                .build_transaction()
                .isolation_level(IsolationLevel::RepeatableRead)
                .read_only(true)
                .start()?;
            let fingerprint = fingerprint(&mut tx)?;
            tx.commit()?;
            Ok(fingerprint)
        })
    }
    pub fn stats(&self) -> Result<MerchantStats> {
        self.run(|client| {
            let mut tx = client
                .build_transaction()
                .isolation_level(IsolationLevel::RepeatableRead)
                .read_only(true)
                .start()?;
            let totals = tx.query_one(STATS_TOTALS, &[])?;
            let mut by_source = Vec::new();
            for row in tx.query(STATS_SOURCES, &[])? {
                by_source.push(MerchantSourceStats {
                    source: row.get(0),
                    merchants: usize::try_from(row.get::<_, i64>(1))?,
                    records: usize::try_from(row.get::<_, i64>(2))?,
                });
            }
            let index = postgres_markets(&mut tx, None)?;
            let merchants = postgres_catalog(&mut tx, &index)?;
            let (without_market_evidence, by_market) = market_counts(&merchants);
            let by_source_region = index.region_stats();
            tx.commit()?;
            Ok(MerchantStats {
                total: usize::try_from(totals.get::<_, i64>(0))?,
                manual: usize::try_from(totals.get::<_, i64>(1))?,
                without_source: usize::try_from(totals.get::<_, i64>(2))?,
                by_source,
                without_market_evidence,
                by_market,
                by_source_region,
            })
        })
    }
    pub fn list(&self, market: Option<&str>, limit: usize, offset: usize) -> Result<MerchantPage> {
        if !(1..=1000).contains(&limit) {
            bail!("limit must be between 1 and 1000");
        }
        if market.is_some_and(|v| v.len() != 2 || !v.bytes().all(|c| c.is_ascii_uppercase())) {
            bail!("market must be a two-letter uppercase code");
        }
        let market = market.map(str::to_owned);
        let sql_offset = i64::try_from(offset)?;
        self.run(move |client| {
            let mut tx = client.build_transaction().isolation_level(IsolationLevel::RepeatableRead).read_only(true).start()?;
            let page = if market.is_some() {
                let index = postgres_markets(&mut tx, None)?;
                paginate(postgres_catalog(&mut tx, &index)?, market.as_deref(), limit, offset)
            } else {
                let total: i64 = tx.query_one("SELECT COUNT(*) FROM merchants", &[])?.get(0);
                let mut merchants = Vec::new();
                for row in tx.query("SELECT data FROM merchants ORDER BY lower(data::jsonb->>'name') COLLATE \"C\",id LIMIT $1 OFFSET $2", &[&(limit as i64), &sql_offset])? {
                    let merchant: Merchant = serde_json::from_str(row.get::<_, &str>(0))?;
                    merchants.push(merchant);
                }
                let ids: Vec<_> = merchants.iter().map(|m| m.id.clone()).collect();
                let index = postgres_markets(&mut tx, Some(&ids))?;
                let merchants = merchants.into_iter().map(|m| index.hydrate(m)).collect();
                MerchantPage { merchants, total: usize::try_from(total)?, limit, offset }
            };
            tx.commit()?;
            Ok(page)
        })
    }
    pub fn search(
        &self,
        description: &str,
        country: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Candidate>> {
        let query = normalize(description);
        if query.is_empty() || limit == 0 {
            return Ok(vec![]);
        }
        let country = country.map(str::to_owned);
        let description = description.to_owned();
        self.run(move |client| {
            let mut tx = client.build_transaction().isolation_level(IsolationLevel::RepeatableRead).read_only(true).start()?;
            let mut found = HashMap::new();
            let mut scorer = crate::search_score::Scorer::new(&query);
            let exact = tx.query("SELECT m.data FROM merchants m JOIN aliases a ON a.merchant_id=m.id WHERE a.normalized=$1 ORDER BY m.id LIMIT 255", &[&query])?;
            for row in exact {
                add_candidate(&mut tx, &mut found, row.get(0), &query, true, None, &mut scorer)?;
            }
            let mut matches: HashMap<String, usize> = HashMap::new();
            for row in tx.query("SELECT s.merchant_id,s.data::jsonb->'raw'->>'transaction_text_regexp' FROM source_records s JOIN merchants m ON m.id=s.merchant_id WHERE s.source='open-enrichment' AND coalesce(s.data::jsonb->'raw'->>'parent_id','')='' AND jsonb_typeof(s.data::jsonb->'raw'->'transaction_text_regexp')='string'", &[])? {
                let id: String = row.get(0);
                if let Some(length) = crate::regex_rules::match_length(row.get(1), &description) {
                    matches.entry(id).and_modify(|v| *v = (*v).max(length)).or_insert(length);
                }
            }
            for (id, length) in matches {
                if !found.contains_key(&id) {
                    let row = tx.query_one("SELECT data FROM merchants WHERE id=$1", &[&id])?;
                    add_candidate(&mut tx, &mut found, row.get(0), &query, false, Some(length), &mut scorer)?;
                }
                if let Some(candidate) = found.get_mut(&id) { candidate.regex_match_length = Some(length); }
            }
            let tokens: Vec<_> = query.split_whitespace().filter(|t| t.chars().count()>=3).take(16).collect();
            let expression = tokens.iter().map(|t| format!("'{t}':*")).collect::<Vec<_>>().join(" | ");
            if !expression.is_empty() {
                for row in tx.query("SELECT m.data FROM merchant_search s JOIN merchants m ON m.id=s.merchant_id WHERE s.tokens @@ to_tsquery('simple',$1) ORDER BY ts_rank(s.tokens,to_tsquery('simple',$1)) DESC,m.id LIMIT 100", &[&expression])? {
                    add_candidate(&mut tx,&mut found,row.get(0),&query,false,None,&mut scorer)?;
                }
            }
            // Match the existing substring-trigram candidate generation. Each
            // LIKE predicate uses pg_trgm's GIN index; Rust retains final scoring.
            let mut grams = HashSet::new();
            for token in tokens {
                let chars: Vec<_> = token.chars().collect();
                for window in chars.windows(3) {
                    if grams.len()<64 { grams.insert(window.iter().collect::<String>()); }
                }
            }
            let mut patterns: Vec<String> = grams.into_iter().map(|g| format!("%{g}%")).collect();
            patterns.sort();
            if !patterns.is_empty() {
                let clauses = (0..patterns.len()).map(|i| format!("s.text LIKE ${}",i+1)).collect::<Vec<_>>().join(" OR ");
                let sql = format!("SELECT m.data FROM merchant_search s JOIN merchants m ON m.id=s.merchant_id WHERE ({clauses}) ORDER BY similarity(s.text,${}) DESC,m.id LIMIT 100",patterns.len()+1);
                let mut params: Vec<&(dyn postgres::types::ToSql + Sync)> = Vec::new();
                for pattern in &patterns { params.push(pattern); }
                params.push(&query);
                for row in tx.query(&sql,&params)? {
                    add_candidate(&mut tx,&mut found,row.get(0),&query,false,None,&mut scorer)?;
                }
            }
            let ids: Vec<_> = found.keys().cloned().collect();
            let index = postgres_markets(&mut tx, Some(&ids))?;
            for candidate in found.values_mut() {
                candidate.merchant = index.hydrate(candidate.merchant.clone());
            }
            tx.commit()?;
            Ok(rank_candidates(found, limit, country.as_deref()))
        })
    }
    pub fn restore(
        &self,
        locations: Vec<(String, String)>,
        merchants: Vec<(String, String)>,
        manual: Vec<(String, String)>,
        sources: Vec<(String, String)>,
        expected_fingerprint: String,
    ) -> Result<usize> {
        self.run(move |client| {
            let mut tx = client.transaction()?;
            write_lock(&mut tx)?;
            let occupied: bool = tx.query_one("SELECT EXISTS(SELECT 1 FROM merchants) OR EXISTS(SELECT 1 FROM manual_merchants) OR EXISTS(SELECT 1 FROM source_records) OR EXISTS(SELECT 1 FROM location_records)",&[])?.get(0);
            if occupied {
                bail!("SQLite migration requires an empty PostgreSQL catalog");
            }
            let count = merchants.len();
            for (id,data) in &merchants {
                let merchant: Merchant = serde_json::from_str(data)?;
                validate(&merchant)?;
                if &merchant.id != id {
                    bail!("SQLite merchant ID does not match its record");
                }
                tx.execute("INSERT INTO merchants(id,data) VALUES($1,$2)",&[id,data])?;
            }
            for (id,data) in manual {
                let merchant: Merchant = serde_json::from_str(&data)?;
                validate(&merchant)?;
                if merchant.id != id {
                    bail!("SQLite manual merchant ID does not match its record");
                }
                tx.execute("INSERT INTO manual_merchants VALUES($1,$2)",&[&id,&data])?;
            }
            for (id,data) in sources {
                let record: SourceRecord = serde_json::from_str(&data)?;
                validate(&record.merchant)?;
                tx.execute("INSERT INTO source_records VALUES($1,$2,$3,$4)",&[&record.source,&record.external_id,&id,&data])?;
            }
            for (id,_) in merchants {
                rebuild(&mut tx,&id)?;
            }
            for (id, data) in locations {
                let record: LocationRecord = serde_json::from_str(&data)?;
                record.validate()?;
                if record.location.id.as_deref() != Some(&id) { bail!("SQLite location ID does not match its record"); }
                let (merchant_id, merchant_source, merchant_external) = location_reference(&record.merchant);
                tx.execute("INSERT INTO location_records(id,source,external_id,merchant_id,merchant_source,merchant_external_id,data) VALUES($1,$2,$3,$4,$5,$6,$7)", &[&id,&record.source,&record.external_id,&merchant_id,&merchant_source,&merchant_external,&data])?;
            }
            if fingerprint(&mut tx)? != expected_fingerprint {
                bail!("migrated catalog differs from SQLite; transaction rolled back");
            }
            tx.commit()?;
            Ok(count)
        })
    }
}
fn migrate_markets(tx: &mut Transaction<'_>) -> Result<()> {
    for table in ["merchants", "manual_merchants"] {
        for row in tx.query(&format!("SELECT id,data FROM {table}"), &[])? {
            let id: String = row.get(0);
            let data = crate::markets::migrate_country(row.get(1), false)?;
            tx.execute(
                &format!("UPDATE {table} SET data=$1 WHERE id=$2"),
                &[&data, &id],
            )?;
        }
    }
    for row in tx.query("SELECT source,external_id,data FROM source_records", &[])? {
        let source: String = row.get(0);
        let external_id: String = row.get(1);
        let data = crate::markets::migrate_country(row.get(2), true)?;
        tx.execute(
            "UPDATE source_records SET data=$1 WHERE source=$2 AND external_id=$3",
            &[&data, &source, &external_id],
        )?;
    }
    tx.batch_execute(include_str!("../migrations/004_markets.sql"))?;
    Ok(())
}

fn write_lock(tx: &mut Transaction<'_>) -> Result<()> {
    tx.query_one("SELECT pg_advisory_xact_lock($1)", &[&WRITE_LOCK])?;
    Ok(())
}
fn source_records(c: &mut impl GenericClient, id: &str) -> Result<Vec<SourceRecord>> {
    Ok(c.query(
        "SELECT data FROM source_records WHERE merchant_id=$1 ORDER BY source,external_id",
        &[&id],
    )?
    .iter()
    .map(|r| serde_json::from_str(r.get::<_, &str>(0)))
    .collect::<std::result::Result<_, _>>()?)
}
fn postgres_markets(tx: &mut impl GenericClient, ids: Option<&[String]>) -> Result<MarketIndex> {
    let mut index = MarketIndex::default();
    let filter = |column: &str| {
        if ids.is_some() {
            format!("{column} = ANY($1::text[])")
        } else {
            "$1::text[] IS NULL".into()
        }
    };
    let manual_sql = format!(
        "SELECT id,data FROM manual_merchants WHERE {}",
        filter("id")
    );
    for row in tx.query(&manual_sql, &[&ids])? {
        index.declaration(
            row.get(0),
            &serde_json::from_str(row.get::<_, &str>(1))?,
            "manual",
            None,
        );
    }
    let sources_sql = format!(
        "SELECT merchant_id,data FROM source_records WHERE {} ORDER BY source,external_id",
        filter("merchant_id")
    );
    for row in tx.query(&sources_sql, &[&ids])? {
        index.source(row.get(0), &serde_json::from_str(row.get::<_, &str>(1))?);
    }
    let outlets_sql = if ids.is_some() {
        format!(
            "SELECT merchant_id,data FROM location_records WHERE {} UNION ALL SELECT s.merchant_id,l.data FROM source_records s JOIN location_records l ON s.source=l.merchant_source AND s.external_id=l.merchant_external_id WHERE l.merchant_id IS NULL AND {}",
            filter("merchant_id"),
            filter("s.merchant_id")
        )
    } else {
        "SELECT COALESCE(l.merchant_id,s.merchant_id),l.data FROM location_records l LEFT JOIN source_records s ON s.source=l.merchant_source AND s.external_id=l.merchant_external_id WHERE $1::text[] IS NULL".into()
    };
    for row in tx.query(&outlets_sql, &[&ids])? {
        index.outlet(row.get(0), &serde_json::from_str(row.get::<_, &str>(1))?);
    }
    Ok(index)
}
fn postgres_catalog(tx: &mut impl GenericClient, index: &MarketIndex) -> Result<Vec<Merchant>> {
    tx.query("SELECT data FROM merchants", &[])?
        .iter()
        .map(|r| Ok(index.hydrate(serde_json::from_str(r.get::<_, &str>(0))?)))
        .collect()
}
fn add_candidate(
    tx: &mut Transaction<'_>,
    found: &mut HashMap<String, Candidate>,
    data: &str,
    query: &str,
    exact: bool,
    regex_match_length: Option<usize>,
    scorer: &mut crate::search_score::Scorer<'_>,
) -> Result<()> {
    let merchant: Merchant = serde_json::from_str(data)?;
    if found.contains_key(&merchant.id) {
        return Ok(());
    }
    let score = if exact || regex_match_length.is_some() {
        1.0
    } else {
        std::iter::once(&merchant.name)
            .chain(merchant.aliases.iter())
            .map(|n| scorer.score(normalize(n)))
            .fold(0.0, f64::max)
    };
    if score < 0.35 {
        return Ok(());
    }
    let manual = tx
        .query_opt(
            "SELECT data FROM manual_merchants WHERE id=$1",
            &[&merchant.id],
        )?
        .map(|r| serde_json::from_str::<Merchant>(r.get::<_, &str>(0)))
        .transpose()?;
    let trusted = manual.is_some_and(|m| {
        std::iter::once(&m.name)
            .chain(m.aliases.iter())
            .any(|n| normalize(n) == query)
    });
    let provenance = source_records(tx, &merchant.id)?;
    if excluded(query, &provenance) && !trusted {
        return Ok(());
    }
    found.insert(
        merchant.id.clone(),
        Candidate {
            merchant,
            score,
            exact,
            regex_match_length,
            trusted,
            provenance,
        },
    );
    Ok(())
}
fn rebuild(tx: &mut Transaction<'_>, id: &str) -> Result<()> {
    let mut merchant = tx
        .query_opt("SELECT data FROM manual_merchants WHERE id=$1", &[&id])?
        .map(|r| serde_json::from_str::<Merchant>(r.get::<_, &str>(0)))
        .transpose()?;
    for record in source_records(tx, id)? {
        if merchant.is_none() {
            merchant = Some(record.merchant.clone());
        }
        let m = merchant.as_mut().unwrap();
        m.aliases
            .extend(std::iter::once(record.merchant.name).chain(record.merchant.aliases));
        if !record.url.is_empty() {
            m.sources.push(record.url);
        }
    }
    tx.execute("DELETE FROM aliases WHERE merchant_id=$1", &[&id])?;
    tx.execute("DELETE FROM merchant_search WHERE merchant_id=$1", &[&id])?;
    let Some(mut merchant) = merchant else {
        tx.execute("DELETE FROM merchants WHERE id=$1", &[&id])?;
        return Ok(());
    };
    merchant.id = id.into();
    merchant.aliases.sort();
    merchant.aliases.dedup();
    merchant.sources.sort();
    merchant.sources.dedup();
    tx.execute("INSERT INTO merchants(id,data) VALUES($1,$2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",&[&id,&serde_json::to_string(&merchant)?])?;
    let mut names: Vec<_> = std::iter::once(&merchant.name)
        .chain(merchant.aliases.iter())
        .map(|n| normalize(n))
        .collect();
    names.sort();
    names.dedup();
    for name in &names {
        tx.execute("INSERT INTO aliases VALUES($1,$2)", &[&id, name])?;
    }
    tx.execute(
        "INSERT INTO merchant_search(merchant_id,text) VALUES($1,$2)",
        &[&id, &names.join(" \n ")],
    )?;
    Ok(())
}

fn fingerprint(tx: &mut Transaction<'_>) -> Result<String> {
    let mut contents = Vec::new();
    for sql in [
        "SELECT data FROM merchants ORDER BY id",
        "SELECT data FROM manual_merchants ORDER BY id",
        "SELECT data,merchant_id FROM source_records ORDER BY source,external_id",
        "SELECT data FROM location_records ORDER BY source,external_id",
    ] {
        for row in tx.query(sql, &[])? {
            for column in 0..row.len() {
                let value: String = row.get(column);
                contents.extend(value.bytes());
                contents.push(0);
            }
        }
    }
    Ok(crate::eval::fingerprint(&contents))
}

#[cfg(test)]
mod connection_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn lazy_store_does_not_connect_during_startup() -> Result<()> {
        // No server exists here. Construction still succeeds inside an async
        // runtime; the public website/health can start without waking Aurora.
        let store = PostgresStore::lazy("postgresql://localhost:1/unused?sslmode=disable")?;
        assert!(store.run(|_| Ok(())).is_err());
        Ok(())
    }

    #[test]
    #[ignore = "requires ULTRAFINANCE_TEST_DATABASE_URL pointing at a disposable PostgreSQL database"]
    fn postgres_idle_session_reconnects_before_work_without_replaying_jobs() -> Result<()> {
        let url = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")?;
        let store = PostgresStore::connect(&url, true)?;
        let first_pid: i32 = store.run(|client| {
            let version: i32 = client
                .query_one("SELECT version FROM ultrafinance_schema", &[])?
                .get(0);
            assert_eq!(
                version, 3,
                "market migration must run before application use"
            );
            let idle: String = client.query_one("SHOW idle_session_timeout", &[])?.get(0);
            assert_eq!(idle, "1min");
            client.batch_execute("SET idle_session_timeout = '1s'")?;
            Ok(client.query_one("SELECT pg_backend_pid()", &[])?.get(0))
        })?;
        std::thread::sleep(Duration::from_secs(2));
        // Server-side expiry means a fresh backend, even though the store and
        // worker remain alive. The caller sees success on its first operation.
        let second_pid: i32 = store.run(|client| {
            let idle: String = client.query_one("SHOW idle_session_timeout", &[])?.get(0);
            assert_eq!(idle, "1min");
            Ok(client.query_one("SELECT pg_backend_pid()", &[])?.get(0))
        })?;
        assert_ne!(first_pid, second_pid);

        let attempts = Arc::new(AtomicUsize::new(0));
        let observed = attempts.clone();
        let result: Result<()> = store.run(move |client| {
            observed.fetch_add(1, Ordering::SeqCst);
            client.batch_execute("CREATE TEMP TABLE completed_write(value int); INSERT INTO completed_write VALUES (1)")?;
            bail!("simulated error after a completed write")
        });
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        let rows: i64 = store.run(|client| {
            Ok(client
                .query_one("SELECT count(*) FROM completed_write", &[])?
                .get(0))
        })?;
        assert_eq!(rows, 1);
        Ok(())
    }
}
