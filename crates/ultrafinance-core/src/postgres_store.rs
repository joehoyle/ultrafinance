//! Synchronous store on a dedicated worker: postgres owns a Tokio runtime and
//! must never be constructed or dropped on the API/CLI's async runtime threads.
use super::{
    Candidate, Merchant, MerchantPage, SourceRecord, excluded, normalize, rank_candidates, validate,
};
use super::{MerchantSourceStats, MerchantStats, STATS_SOURCES, STATS_TOTALS, location_reference};
use crate::location::LocationRecord;
use crate::markets::MarketCountries;
use anyhow::{Context, Result, bail};
use postgres::{Client, GenericClient, IsolationLevel, Transaction};
use postgres_native_tls::MakeTlsConnector;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, mpsc},
    time::Duration,
};

#[path = "postgres_store/import_bulk.rs"]
mod import_bulk;
#[path = "postgres_store/indexed_import.rs"]
mod indexed_import;
#[path = "postgres_store/foursquare_locations.rs"]
mod foursquare_locations;
#[path = "postgres_store/typed_sources.rs"]
mod typed_sources;

type Job = Box<dyn FnOnce(Result<&mut Client>) + Send>;
#[derive(Clone)]
pub(super) struct PostgresStore(Arc<Worker>);
struct Worker {
    sender: Option<mpsc::Sender<Job>>,
    thread: Option<std::thread::JoinHandle<()>>,
    temporary: Option<TemporaryDatabase>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.temporary.take();
    }
}
struct TemporaryDatabase {
    admin: postgres::Config,
    name: String,
    url: String,
}
impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        let config = self.admin.clone();
        let name = self.name.clone();
        // Native postgres owns a runtime; construct and drop it off async callers.
        let _ = std::thread::spawn(move || -> Result<()> {
            let tls = MakeTlsConnector::new(native_tls::TlsConnector::builder().build()?);
            let mut client = config.connect(tls)?;
            client.batch_execute(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))?;
            Ok(())
        })
        .join();
    }
}
const WRITE_LOCK: i64 = 0x756c74726166696e;

impl PostgresStore {
    pub fn temporary(url: &str) -> Result<Self> {
        let name = format!("ultrafinance_test_{}", uuid::Uuid::new_v4().simple());
        let mut scoped =
            reqwest::Url::parse(url).context("temporary catalogs require a PostgreSQL URL")?;
        scoped.set_path(&name);
        let mut admin: postgres::Config = url
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid PostgreSQL test configuration"))?;
        admin.connect_timeout(Duration::from_secs(35));
        let config = admin.clone();
        let database = name.clone();
        std::thread::spawn(move || -> Result<()> {
            let tls = MakeTlsConnector::new(native_tls::TlsConnector::builder().build()?);
            let mut client = config.connect(tls).context("local PostgreSQL unavailable; run dev/postgres.sh up or set ULTRAFINANCE_TEST_DATABASE_URL")?;
            client.batch_execute(&format!("CREATE DATABASE \"{database}\" TEMPLATE template0"))?;
            Ok(())
        }).join().map_err(|_|anyhow::anyhow!("PostgreSQL fixture worker failed"))??;
        let temporary = TemporaryDatabase {
            admin,
            name,
            url: scoped.to_string(),
        };
        Self::connect_with_lease(&temporary.url.clone(), true, false, Some(temporary))
    }
    pub fn temporary_url(&self) -> Option<&str> {
        self.0.temporary.as_ref().map(|t| t.url.as_str())
    }
    pub fn get(&self, id: &str) -> Result<Option<Merchant>> {
        let id = id.to_owned();
        self.run(move |c| {
            let mut tx = c.transaction()?;
            let row = tx.query_opt("SELECT data FROM merchants_documents WHERE id=$1", &[&id])?;
            row.map(|r| serde_json::from_str(r.get(0)).map_err(Into::into))
                .transpose()
        })
    }
    pub fn resolutions(
        &self,
        id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::resolution::Resolution>> {
        let id = id.map(str::to_owned);
        self.run(move |c| {
            c.query("SELECT data FROM descriptor_resolution_documents WHERE ($1::text IS NULL OR id=$1) ORDER BY id LIMIT $2", &[&id,&(limit as i64)])?
                .into_iter().map(|r|Ok(serde_json::from_str(r.get(0))?)).collect()
        })
    }
    pub fn save_resolution(&self, resolution: &crate::resolution::Resolution) -> Result<bool> {
        let resolution = resolution.clone();
        self.run(move |c| {
            let mut tx = c.transaction()?;
            write_lock(&mut tx)?;
            let old = tx
                .query_opt(
                    "SELECT data FROM descriptor_resolution_documents WHERE id=$1",
                    &[&resolution.id],
                )?
                .map(|r| serde_json::from_str::<crate::resolution::Resolution>(r.get(0)))
                .transpose()?;
            crate::resolution::check_update(old.as_ref(), &resolution)?;
            crate::columns::postgres_resolutions(
                &mut tx,
                &resolution.id,
                &serde_json::to_string(&resolution)?,
            )?;
            tx.commit()?;
            Ok(true)
        })
    }
    pub fn revoke_resolution(&self, id: &str) -> Result<bool> {
        let id = id.to_owned();
        self.run(move |c| {
            Ok(c.execute("DELETE FROM descriptor_resolutions WHERE id=$1", &[&id])? > 0)
        })
    }
    pub fn connect(url: &str, initialize: bool) -> Result<Self> {
        Self::connect_with_mode(url, initialize, false)
    }
    pub fn lazy(url: &str) -> Result<Self> {
        Self::connect_with_mode(url, false, true)
    }
    fn connect_with_mode(url: &str, initialize: bool, lazy: bool) -> Result<Self> {
        Self::connect_with_lease(url, initialize, lazy, None)
    }
    fn connect_with_lease(
        url: &str,
        initialize: bool,
        lazy: bool,
        temporary: Option<TemporaryDatabase>,
    ) -> Result<Self> {
        // Parse outside the worker, but never attach the URL to diagnostics.
        let mut config: postgres::Config = url
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid PostgreSQL connection configuration"))?;
        config.connect_timeout(Duration::from_secs(35));
        config.application_name("ultrafinance");
        let (sender, receiver) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new().name("merchant-postgres".into()).spawn(move || {
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
                    tx.batch_execute(crate::dedupe::SCHEMA)?;
                    tx.batch_execute(crate::location_dedupe::SCHEMA)?;
                    if version > 8 { bail!("unsupported PostgreSQL schema version {version}"); }
                    if version < 4 {
                        tx.batch_execute(crate::resolution::LEGACY_SCHEMA)?;
                        tx.batch_execute(include_str!("../migrations/005_columns_postgres.sql"))?;
                        crate::resolution::migrate_postgres(&mut tx)?;
                    }
                    if version < 5 {
                        tx.batch_execute("SET LOCAL statement_timeout='0'; SET LOCAL idle_in_transaction_session_timeout='0'")?;
                        tx.batch_execute(include_str!("../migrations/006_typed_source_inputs.sql"))?;
                    }
                    if version < 6 {
                        tx.batch_execute("SET LOCAL statement_timeout='0'; SET LOCAL idle_in_transaction_session_timeout='0'")?;
                        tx.batch_execute(include_str!("../migrations/007_source_inputs_only.sql"))?;
                    }
                    if version < 7 {
                        tx.batch_execute("SET LOCAL statement_timeout='0'; SET LOCAL idle_in_transaction_session_timeout='0'")?;
                        tx.batch_execute(include_str!("../migrations/008_merchant_identity_index.sql"))?;
                        indexed_import::backfill(&mut tx)?;
                    }
                    if version < 8 {
                        tx.batch_execute("SET LOCAL statement_timeout='0'; SET LOCAL idle_in_transaction_session_timeout='0'")?;
                        tx.batch_execute("UPDATE ultrafinance_schema SET version=8")?;
                    }
                    if version < 4 { refresh_market_lookup(&mut tx,None)?; }
                    tx.commit()?;
                }
                let version: i32 = client.query_one("SELECT version FROM ultrafinance_schema", &[])
                    .context("PostgreSQL schema missing; run `ultrafinance database init` first")?.get(0);
                if version != 8 { bail!("unsupported PostgreSQL schema version {version}; run `ultrafinance database init` to migrate lookup columns"); }
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
        Ok(Self(Arc::new(Worker {
            sender: Some(sender),
            thread: Some(thread),
            temporary,
        })))
    }
    fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Client) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.0
            .sender
            .as_ref()
            .expect("live worker sender")
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
            let redirects: bool = tx.query_one("SELECT to_regclass('public.merchant_redirects') IS NOT NULL", &[])?.get(0);
            for mut record in records {
                if redirects && let crate::location::MerchantReference::Local {merchant_id} = &mut record.merchant
                    && let Some(row)=tx.query_opt("SELECT merchant_id FROM merchant_redirects WHERE retired_id=$1", &[&*merchant_id])? { *merchant_id=row.get(0); }
                let existing: Option<String> = tx.query_opt("SELECT data FROM location_records_documents WHERE source=$1 AND external_id=$2", &[&record.source,&record.external_id])?.map(|r| r.get(0));
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
                crate::columns::postgres_location_records(&mut tx, &id, &record.source, &record.external_id, &merchant_id, &merchant_source, &merchant_external, &serde_json::to_string(&stored)?)?;
            }
            reconcile_locations(&mut tx, None, false)?;
            refresh_market_lookup(&mut tx, None)?;
            tx.commit()?;
            Ok(())
        })
    }
    pub fn locations(&self, merchant_id: &str) -> Result<Vec<LocationRecord>> {
        let merchant_id = merchant_id.to_owned();
        self.run(move |client| {
            let mut tx = client
                .build_transaction()
                .isolation_level(IsolationLevel::RepeatableRead)
                .read_only(true)
                .start()?;
            let records = location_rows(&mut tx, Some(&merchant_id))?
                .into_iter()
                .map(|(_, r)| r)
                .collect();
            let output =
                crate::location_dedupe::consolidate(records, &location_redirects(&mut tx)?);
            tx.commit()?;
            Ok(output)
        })
    }
    pub fn location_sources(&self, merchant_id: &str) -> Result<Vec<LocationRecord>> {
        let merchant_id = merchant_id.to_owned();
        self.run(move |client| {
            Ok(location_rows(client, Some(&merchant_id))?
                .into_iter()
                .map(|(_, record)| record)
                .collect())
        })
    }
    pub fn dedupe_locations(
        &self,
        merchant_id: Option<&str>,
        dry_run: bool,
    ) -> Result<crate::location_dedupe::Report> {
        let merchant_id = merchant_id.map(str::to_owned);
        self.run(move |client| {
            let mut tx = client.transaction()?;
            write_lock(&mut tx)?;
            tx.batch_execute(
                "LOCK TABLE merchants,source_records,location_records IN SHARE ROW EXCLUSIVE MODE",
            )?;
            let report = reconcile_locations(&mut tx, merchant_id.as_deref(), dry_run)?;
            tx.commit()?;
            Ok(report)
        })
    }

    pub fn put(&self, merchant: &Merchant) -> Result<()> {
        validate(merchant)?;
        let merchant = merchant.clone();
        self.run(move |client| {
            let mut tx = client.transaction()?;
            write_lock(&mut tx)?;
            let redirects: bool = tx
                .query_one(
                    "SELECT to_regclass('public.merchant_redirects') IS NOT NULL",
                    &[],
                )?
                .get(0);
            if redirects
                && tx
                    .query_opt(
                        "SELECT retired_id FROM merchant_redirects WHERE retired_id=$1",
                        &[&merchant.id],
                    )?
                    .is_some()
            {
                bail!("merchant ID was retired by dedupe; use its surviving ID");
            }
            crate::columns::postgres_manual_merchants(
                &mut tx,
                &merchant.id,
                &serde_json::to_string(&merchant)?,
            )?;
            rebuild(&mut tx, &merchant.id)?;
            tx.commit()?;
            Ok(())
        })
    }
    pub fn source_records(
        &self,
        source: &str,
        external_id: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<serde_json::Value>> {
        let (source, external_id) = (source.to_owned(), external_id.map(str::to_owned));
        let (limit, offset) = (i64::try_from(limit)?, i64::try_from(offset)?);
        self.run(move |client| {
            client.query("SELECT merchant_id,data FROM source_records_documents WHERE source=$1 AND ($2::text IS NULL OR external_id=$2) ORDER BY external_id LIMIT $3 OFFSET $4", &[&source,&external_id,&limit,&offset])?.into_iter().map(|row| {
                Ok(serde_json::json!({"merchant_id": row.get::<_,String>(0), "record": serde_json::from_str::<serde_json::Value>(&row.get::<_,String>(1))?}))
            }).collect()
        })
    }
    pub fn import(&self, records: &[SourceRecord]) -> Result<super::ImportDelta> {
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
            let delta = import_records(&mut tx, &records, None, None)?;
            foursquare_locations::import(&mut tx, &records)?;
            tx.commit()?;
            Ok(delta)
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
    pub fn dedupe_candidates(
        &self,
        max_pairs: usize,
    ) -> Result<(crate::dedupe::Snapshot, Vec<(usize, usize)>)> {
        self.run(move |client| {
            use postgres::fallible_iterator::FallibleIterator;
            let mut tx=client.build_transaction().isolation_level(IsolationLevel::RepeatableRead).read_only(true).start()?;
            // Trigrams supply candidates only; Rust applies the same edit-distance
            // threshold as before. Exact names/aliases and hosts have B-tree indexes.
            tx.batch_execute("SET LOCAL pg_trgm.similarity_threshold='0.000001'; SET LOCAL statement_timeout='0'")?;
            let mut pairs=Vec::new();
            let mut seen=HashSet::new();
            {
                let mut rows=tx.query_raw(r#"WITH candidates AS (
SELECT a.merchant_id AS left_id,b.merchant_id AS right_id,TRUE AS exact FROM merchant_identity_keys a JOIN merchant_identity_keys b ON a.rule_name=b.rule_name AND a.rule_host=b.rule_host AND a.merchant_id<b.merchant_id WHERE a.rule_name IS NOT NULL
UNION ALL
SELECT a.merchant_id,b.merchant_id,TRUE FROM aliases a JOIN aliases b ON a.normalized=b.normalized AND a.merchant_id<b.merchant_id WHERE length(a.normalized)>=3
UNION ALL
SELECT a.merchant_id,b.merchant_id,TRUE FROM merchant_identity_keys a JOIN merchant_identity_keys b ON a.website_host=b.website_host AND a.merchant_id<b.merchant_id WHERE a.website_host IS NOT NULL
UNION ALL
SELECT a.merchant_id,b.merchant_id,FALSE FROM merchant_identity_keys a JOIN merchant_identity_keys b ON b.normalized_name % a.normalized_name AND a.merchant_id<b.merchant_id WHERE length(a.normalized_name)>=3 AND length(b.normalized_name)>=3)
SELECT c.left_id,c.right_id,c.exact,a.normalized_name,b.normalized_name FROM candidates c JOIN merchant_identity_keys a ON a.merchant_id=c.left_id JOIN merchant_identity_keys b ON b.merchant_id=c.right_id"#,std::iter::empty::<&str>())?;
                while let Some(row)=rows.next()? {
                    let pair=(row.get::<_,String>(0),row.get::<_,String>(1));
                    if seen.contains(&pair) {continue;}
                    let a:String=row.get(3);let b:String=row.get(4);
                    if row.get::<_,bool>(2) || rapidfuzz::distance::levenshtein::normalized_similarity(a.chars(),b.chars())>=0.85 {
                        seen.insert(pair.clone());
                        pairs.push(pair);
                        if pairs.len()>max_pairs {bail!("candidate pairs exceed --max-pairs {max_pairs}; increase the limit to evaluate the complete scan");}
                    }
                }
            }
            pairs.sort();
            let ids:Vec<_>=pairs.iter().flat_map(|(a,b)|[a.clone(),b.clone()]).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
            let snapshot=postgres_snapshot_for_ids(&mut tx,&ids)?;
            let index:HashMap<_,_>=snapshot.merchants.iter().enumerate().map(|(i,m)|(m.id.clone(),i)).collect();
            let pairs=pairs.into_iter().map(|(a,b)|(index[&a],index[&b])).collect();
            tx.commit()?;
            Ok((snapshot,pairs))
        })
    }
    #[cfg(test)]
    pub fn dedupe_snapshot(&self) -> Result<crate::dedupe::Snapshot> {
        self.run(|client| {
            let mut tx = client
                .build_transaction()
                .isolation_level(IsolationLevel::RepeatableRead)
                .read_only(true)
                .start()?;
            let snapshot = postgres_dedupe_snapshot(&mut tx)?;
            tx.commit()?;
            Ok(snapshot)
        })
    }
    pub fn resolve_merchant_id(&self, id: &str) -> Result<String> {
        let id = id.to_owned();
        self.run(move |c| {
            let active: bool = c
                .query_one("SELECT EXISTS(SELECT 1 FROM merchants WHERE id=$1)", &[&id])?
                .get(0);
            if active {
                return Ok(id);
            }
            let exists: bool = c
                .query_one(
                    "SELECT to_regclass('public.merchant_redirects') IS NOT NULL",
                    &[],
                )?
                .get(0);
            if !exists {
                return Ok(id);
            }
            Ok(c.query_opt(
                "SELECT merchant_id FROM merchant_redirects WHERE retired_id=$1",
                &[&id],
            )?
            .map(|r| r.get(0))
            .unwrap_or(id))
        })
    }
    pub fn apply_dedupe(
        &self,
        expected: &crate::dedupe::Snapshot,
        groups: &[Vec<String>],
        audit: &serde_json::Value,
    ) -> Result<String> {
        let (expected, groups, audit) = (expected.clone(), groups.to_vec(), audit.clone());
        self.run(move |client| {
            let mut tx = client.transaction()?;
            write_lock(&mut tx)?;
            tx.batch_execute(crate::dedupe::SCHEMA).context("cannot initialize dedupe tables; run database init with a schema-owner connection")?;
            // Also exclude writers that do not participate in the application's advisory lock.
            tx.batch_execute("LOCK TABLE merchants,manual_merchants,source_records,location_records,merchant_redirects IN SHARE ROW EXCLUSIVE MODE")?;
            let ids:Vec<_>=expected.merchants.iter().map(|m|m.id.clone()).collect();
            let current = postgres_snapshot_for_ids(&mut tx, &ids)?;
            crate::dedupe::validate_plan(&expected, &current, &groups)?;
            let run_id = uuid::Uuid::new_v4().to_string();
            tx.execute("INSERT INTO merchant_merge_runs VALUES($1,$2)", &[&run_id,&serde_json::to_string(&audit)?])?;
            merge_groups(&mut tx, &groups)?;
            tx.commit()?;
            Ok(run_id)
        })
    }
    pub fn reconcile_import(
        &self,
        records: Vec<SourceRecord>,
        dry_run: bool,
        chunk_size: usize,
    ) -> Result<crate::dedupe::ImportReport> {
        if chunk_size == 0 { bail!("import chunk size must be greater than zero"); }
        self.run(move |client| {
            let mut tx = import_transaction(client)?;
            write_lock(&mut tx)?;
            tx.batch_execute("LOCK TABLE merchants,manual_merchants,source_records,location_records,descriptor_resolutions,merchant_redirects IN SHARE ROW EXCLUSIVE MODE")?;
            let mut importer = indexed_import::Import::new(&mut tx, dry_run, records.len(), chunk_size)?;
            let mut records = records.into_iter();
            loop {
                let chunk: Vec<_> = records.by_ref().take(chunk_size).collect();
                if chunk.is_empty() { break; }
                importer.chunk(chunk)?;
            }
            let result = importer.finish()?;
            if dry_run {
                tx.rollback()?;
                eprintln!("Import: identity preview complete; no database changes");
            } else {
                eprintln!("Import: committing merchant and location changes");
                tx.commit()?;
                eprintln!("Import: committed — {} added, {} updated, {} unchanged",result.delta.added,result.delta.updated,result.delta.unchanged);
            }
            Ok(result)
        })
    }
    pub fn reconcile_file(
        &self,
        path: std::path::PathBuf,
        limit: Option<u32>,
        dry_run: bool,
        chunk_size: usize,
    ) -> Result<(crate::dedupe::ImportReport, usize, usize)> {
        if chunk_size == 0 { bail!("import chunk size must be greater than zero"); }
        self.run(move |client| {
            let total = crate::datasets::count_records(&path)?;
            let selected_total = limit.map_or(total, |n| total.min(n as usize));
            eprintln!("Import: {total} available source records; {selected_total} selected");
            eprintln!("Import: waiting for database write lock");
            let mut tx=import_transaction(client)?;
            write_lock(&mut tx)?;
            tx.batch_execute("LOCK TABLE merchants,manual_merchants,source_records,location_records,descriptor_resolutions,merchant_redirects IN SHARE ROW EXCLUSIVE MODE")?;
            let mut importer=indexed_import::Import::new(&mut tx,dry_run,selected_total,chunk_size)?;
            let (available,selected)=crate::datasets::stream_records(&path,limit,chunk_size,|chunk|importer.chunk(chunk))?;
            if (available, selected) != (total, selected_total) { bail!("prepared record counts changed while importing; retry with a stable bundle"); }
            let result=importer.finish()?;
            if dry_run {tx.rollback()?;eprintln!("Import: identity preview complete; no database changes");}
            else {eprintln!("Import: committing merchant and location changes");tx.commit()?;eprintln!("Import: committed — {} added, {} updated, {} unchanged",result.delta.added,result.delta.updated,result.delta.unchanged);}
            Ok((result,available,selected))
        })
    }
    #[cfg(test)]
    pub fn apply_reconciled_import(
        &self,
        expected: &crate::dedupe::Snapshot,
        staged: &crate::dedupe::Snapshot,
        records: &[SourceRecord],
        identities: &[String],
        groups: &[Vec<String>],
        audit: &serde_json::Value,
    ) -> Result<(super::ImportDelta, Option<String>)> {
        let (expected, staged, records, identities, groups, audit) = (
            expected.clone(),
            staged.clone(),
            records.to_vec(),
            identities.to_vec(),
            groups.to_vec(),
            audit.clone(),
        );
        if records.len() != identities.len() {
            bail!("invalid import identity plan");
        }
        self.run(move |client| {
            crate::dedupe::validate_plan(&staged, &staged, &groups)?;
            let mut tx = import_transaction(client)?;
            eprintln!("Import: waiting for database write lock");
            write_lock(&mut tx)?;
            eprintln!("Import: validating catalog snapshot");
            tx.batch_execute(crate::dedupe::SCHEMA).context("cannot initialize dedupe tables; run database init with a schema-owner connection")?;
            tx.batch_execute("LOCK TABLE merchants,manual_merchants,source_records,location_records,descriptor_resolutions,merchant_redirects IN SHARE ROW EXCLUSIVE MODE")?;
            let current = postgres_dedupe_snapshot(&mut tx)?;
            crate::dedupe::validate_plan(&expected, &current, &[])?;
            // New duplicate source records go directly to their survivor. Only
            // already-persisted identities need retirement and redirects.
            let targets: HashMap<_,_> = groups.iter().flat_map(|g| g.iter().map(|id| (id, &g[0]))).collect();
            let identities: Vec<_> = identities.iter().map(|id| targets.get(id).copied().unwrap_or(id).clone()).collect();
            let existing: HashSet<_> = expected.merchants.iter().map(|m| &m.id).collect();
            let persisted_groups: Vec<Vec<String>> = groups.iter().filter_map(|g| {
                let mut group=vec![g[0].clone()];
                group.extend(g[1..].iter().filter(|id| existing.contains(id)).cloned());
                (group.len()>1).then_some(group)
            }).collect();
            let delta = import_records(&mut tx, &records, Some(&identities), Some(&expected))?;
            let run_id = if groups.is_empty() { None } else {
                let run_id = uuid::Uuid::new_v4().to_string();
                tx.execute("INSERT INTO merchant_merge_runs VALUES($1,$2)", &[&run_id,&serde_json::to_string(&audit)?])?;
                merge_groups(&mut tx, &persisted_groups)?;
                Some(run_id)
            };
            eprintln!("Import: committing transaction");
            tx.commit()?;
            eprintln!("Import: committed — {} added, {} updated, {} unchanged", delta.added, delta.updated, delta.unchanged);
            Ok((delta, run_id))
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
            // Coverage is maintained transactionally by catalog writes. Stats
            // needs only aggregate rows, never a hydrated catalog or source payloads.
            let without_markets = usize::try_from(tx.query_one(
                "SELECT count(*) FROM merchants m WHERE COALESCE(m.markets_json,'[]')::jsonb='[]'::jsonb",
                &[],
            )?.get::<_, i64>(0))?;
            let by_market = tx.query(
                "SELECT country,count(DISTINCT m.id) FROM merchants m CROSS JOIN LATERAL jsonb_array_elements_text(COALESCE(m.markets_json,'[]')::jsonb) country GROUP BY country ORDER BY count(DISTINCT m.id) DESC,country",
                &[],
            )?.into_iter().map(|r| Ok(super::MerchantMarketStats {
                market:r.get(0), merchants:usize::try_from(r.get::<_,i64>(1))?,
            })).collect::<Result<Vec<_>>>()?;
            let by_source_region = tx.query(
                "SELECT source,dataset_region,count(DISTINCT merchant_id),count(*) FROM source_records WHERE dataset_region IS NOT NULL GROUP BY source,dataset_region ORDER BY source,dataset_region",
                &[],
            )?.into_iter().map(|r| Ok(super::MerchantRegionStats {
                source:r.get(0),region:r.get(1),merchants:usize::try_from(r.get::<_,i64>(2))?,records:usize::try_from(r.get::<_,i64>(3))?,
            })).collect::<Result<Vec<_>>>()?;
            tx.commit()?;
            Ok(MerchantStats {
                total: usize::try_from(totals.get::<_, i64>(0))?,
                manual: usize::try_from(totals.get::<_, i64>(1))?,
                without_source: usize::try_from(totals.get::<_, i64>(2))?,
                by_source,
                without_markets,
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
            let total: i64 = tx.query_one("SELECT COUNT(*) FROM merchants m WHERE ($1::text IS NULL OR m.markets_json::jsonb ? $1)", &[&market])?.get(0);
            let mut merchants = Vec::new();
            for row in tx.query("SELECT data FROM merchants_documents m WHERE ($1::text IS NULL OR m.markets_json::jsonb ? $1) ORDER BY lower(name) COLLATE \"C\",id LIMIT $2 OFFSET $3", &[&market,&(limit as i64),&sql_offset])? {
                merchants.push(serde_json::from_str::<Merchant>(row.get(0))?);
            }
            let page = MerchantPage {merchants,total:usize::try_from(total)?,limit,offset};
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
            let exact = tx.query("SELECT m.data FROM merchants_documents m JOIN aliases a ON a.merchant_id=m.id WHERE a.normalized=$1 ORDER BY m.id LIMIT 255", &[&query])?;
            for row in exact {
                add_candidate(&mut tx, &mut found, row.get(0), &query, true, None, &mut scorer)?;
            }
            let mut matches: HashMap<String, usize> = HashMap::new();
            for row in tx.query("SELECT s.merchant_id,s.transaction_pattern FROM source_records s JOIN merchants m ON m.id=s.merchant_id WHERE s.source='open-enrichment' AND coalesce(s.parent_id,'')='' AND s.transaction_pattern IS NOT NULL", &[])? {
                let id: String = row.get(0);
                if let Some(length) = crate::regex_rules::match_length(row.get(1), &description) {
                    matches.entry(id).and_modify(|v| *v = (*v).max(length)).or_insert(length);
                }
            }
            for (id, length) in matches {
                if !found.contains_key(&id) {
                    let row = tx.query_one("SELECT data FROM merchants_documents WHERE id=$1", &[&id])?;
                    add_candidate(&mut tx, &mut found, row.get(0), &query, false, Some(length), &mut scorer)?;
                }
                if let Some(candidate) = found.get_mut(&id) { candidate.regex_match_length = Some(length); }
            }
            let tokens: Vec<_> = query.split_whitespace().filter(|t| t.chars().count()>=3).take(16).collect();
            let expression = tokens.iter().map(|t| format!("'{t}':*")).collect::<Vec<_>>().join(" | ");
            if !expression.is_empty() {
                for row in tx.query("SELECT m.data FROM merchant_search s JOIN merchants_documents m ON m.id=s.merchant_id WHERE s.tokens @@ to_tsquery('simple',$1) ORDER BY ts_rank(s.tokens,to_tsquery('simple',$1)) DESC,m.id LIMIT 100", &[&expression])? {
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
                let sql = format!("SELECT m.data FROM merchant_search s JOIN merchants_documents m ON m.id=s.merchant_id WHERE ({clauses}) ORDER BY similarity(s.text,${}) DESC,m.id LIMIT 100",patterns.len()+1);
                let mut params: Vec<&(dyn postgres::types::ToSql + Sync)> = Vec::new();
                for pattern in &patterns { params.push(pattern); }
                params.push(&query);
                for row in tx.query(&sql,&params)? {
                    add_candidate(&mut tx,&mut found,row.get(0),&query,false,None,&mut scorer)?;
                }
            }
            tx.commit()?;
            Ok(rank_candidates(found, limit, country.as_deref()))
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
        "SELECT data FROM source_records_documents WHERE merchant_id=$1 ORDER BY source,external_id",
        &[&id],
    )?
    .iter()
    .map(|r| serde_json::from_str(r.get::<_, &str>(0)))
    .collect::<std::result::Result<_, _>>()?)
}
fn postgres_markets(tx: &mut impl GenericClient, ids: Option<&[String]>) -> Result<MarketCountries> {
    let mut index = MarketCountries::default();
    let filter = |column: &str| {
        if ids.is_some() {
            format!("{column} = ANY($1::text[])")
        } else {
            "$1::text[] IS NULL".into()
        }
    };
    let manual_sql = format!(
        "SELECT id,data FROM manual_merchants_documents WHERE {}",
        filter("id")
    );
    for row in tx.query(&manual_sql, &[&ids])? {
        index.declaration(
            row.get(0),
            &serde_json::from_str(row.get::<_, &str>(1))?,
        );
    }
    for input in typed_sources::read(tx, ids)? {
        index.source_fields(
            &input.merchant_id,
            &input.merchant,
            &input.hints,
            input.region.as_deref(),
        );
    }
    postgres_market_outlets(tx, &mut index, ids)?;
    Ok(index)
}
fn postgres_market_outlets(
    tx: &mut impl GenericClient,
    index: &mut MarketCountries,
    ids: Option<&[String]>,
) -> Result<()> {
    use postgres::fallible_iterator::FallibleIterator;
    // Coverage needs only country and provenance, not full address documents.
    let sql = if ids.is_some() {
        "SELECT DISTINCT merchant_id,country FROM location_records WHERE merchant_id=ANY($1::text[]) AND country IS NOT NULL UNION SELECT s.merchant_id,l.country FROM source_records s JOIN location_records l ON s.source=l.merchant_source AND s.external_id=l.merchant_external_id WHERE l.merchant_id IS NULL AND s.merchant_id=ANY($1::text[]) AND l.country IS NOT NULL"
    } else {
        "SELECT DISTINCT COALESCE(l.merchant_id,s.merchant_id),l.country FROM location_records l LEFT JOIN source_records s ON s.source=l.merchant_source AND s.external_id=l.merchant_external_id WHERE $1::text[] IS NULL AND l.country IS NOT NULL"
    };
    let mut rows = tx.query_raw(sql, [&ids as &(dyn postgres::types::ToSql + Sync)])?;
    while let Some(row) = rows.next()? {
        index.outlet_country(row.get(0), row.get(1));
    }
    Ok(())
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
            "SELECT data FROM manual_merchants_documents WHERE id=$1",
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
            resolution_id: None,
            pending_import: false,
            interpretation_evidence: vec![],
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
    rebuild_with_market_refresh(tx, id, true)
}
fn rebuild_with_market_refresh(
    tx: &mut Transaction<'_>,
    id: &str,
    refresh_markets: bool,
) -> Result<()> {
    let mut merchant = tx
        .query_opt(
            "SELECT data FROM manual_merchants_documents WHERE id=$1",
            &[&id],
        )?
        .map(|r| serde_json::from_str::<Merchant>(r.get::<_, &str>(0)))
        .transpose()?;
    typed_sources::for_each(tx, &[id.to_owned()], |record| {
        if merchant.is_none() {
            merchant = Some(record.merchant.clone());
        }
        let m = merchant.as_mut().unwrap();
        crate::dedupe::combine(m, &record.merchant);
        m.aliases
            .extend(std::iter::once(record.merchant.name).chain(record.merchant.aliases));
        if !record.url.is_empty() {
            m.sources.push(record.url);
        }
        Ok(())
    })?;
    tx.execute("DELETE FROM aliases WHERE merchant_id=$1", &[&id])?;
    tx.execute("DELETE FROM merchant_search WHERE merchant_id=$1", &[&id])?;
    let Some(mut merchant) = merchant else {
        let redirects: bool = tx
            .query_one(
                "SELECT to_regclass('public.merchant_redirects') IS NOT NULL",
                &[],
            )?
            .get(0);
        if redirects
            && tx
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM merchant_redirects WHERE merchant_id=$1)",
                    &[&id],
                )?
                .get::<_, bool>(0)
        {
            bail!(
                "cannot remove a merchant referenced by retired IDs; use dedupe to merge its whole identity"
            );
        }
        tx.execute("DELETE FROM merchants WHERE id=$1", &[&id])?;
        return Ok(());
    };
    merchant.id = id.into();
    merchant.aliases.sort();
    merchant.aliases.dedup();
    merchant.sources.sort();
    merchant.sources.dedup();
    crate::columns::postgres_merchants(tx, id, &serde_json::to_string(&merchant)?)?;
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
    if refresh_markets {
        refresh_market_lookup(tx, Some(&[id.to_owned()]))?;
    }
    Ok(())
}

fn refresh_market_lookup(c: &mut impl GenericClient, ids: Option<&[String]>) -> Result<()> {
    if ids.is_none() {
        let mut after=String::new();
        loop {
            let batch:Vec<String>=c.query("SELECT id FROM merchants WHERE id>$1 ORDER BY id LIMIT 1000",&[&after])?.into_iter().map(|r|r.get(0)).collect();
            if batch.is_empty() {return Ok(());}
            after=batch.last().unwrap().clone();
            refresh_market_lookup(c,Some(&batch))?;
        }
    }
    let index = postgres_markets(c, ids)?;
    let ids = match ids {
        Some(ids) => ids.to_vec(),
        None => c
            .query("SELECT id FROM merchants", &[])?
            .into_iter()
            .map(|r| r.get(0))
            .collect(),
    };
    for id in ids {
        if let Some(row) = c.query_opt("SELECT data FROM merchants_documents WHERE id=$1", &[&id])? {
            let merchant: Merchant = serde_json::from_str(row.get(0))?;
            let markets = serde_json::to_string(&index.hydrate(merchant).markets)?;
            c.execute("UPDATE merchants SET markets_json=$2 WHERE id=$1 AND markets_json IS DISTINCT FROM $2", &[&id,&markets])?;
        }
    }

    Ok(())
}

fn fingerprint(tx: &mut Transaction<'_>) -> Result<String> {
    use postgres::fallible_iterator::FallibleIterator;
    let mut hash=0xcbf29ce484222325u64;
    let mut feed=|bytes:&[u8]| {
        for byte in bytes.iter().copied().chain(std::iter::once(0)) {
            hash^=u64::from(byte);hash=hash.wrapping_mul(0x100000001b3);
        }
    };
    for sql in [
        "SELECT data FROM merchants_documents ORDER BY id",
        "SELECT data FROM manual_merchants_documents ORDER BY id",
        "SELECT data,merchant_id FROM source_records_documents ORDER BY source,external_id",
        "SELECT data FROM location_records_documents ORDER BY source,external_id",
    ] {
        let mut rows=tx.query_raw(sql,std::iter::empty::<&str>())?;
        while let Some(row)=rows.next()? {
            for column in 0..row.len() {
                let value: String = row.get(column);
                let value = if column == 0 {
                    crate::columns::canonical_document(&value)?
                } else {
                    value
                };
                feed(value.as_bytes());
            }
        }
    }
    let exists: bool = tx
        .query_one(
            "SELECT to_regclass('public.descriptor_resolutions') IS NOT NULL",
            &[],
        )?
        .get(0);
    if exists {
        let mut rows=tx.query_raw("SELECT data FROM descriptor_resolution_documents ORDER BY id",std::iter::empty::<&str>())?;
        while let Some(row)=rows.next()? {
            let data: String = row.get(0);
            feed(crate::columns::canonical_document(&data)?.as_bytes());
        }
    }
    let mut rows=tx.query_raw("SELECT retired_id,location_id FROM location_redirects ORDER BY retired_id",std::iter::empty::<&str>())?;
    while let Some(row)=rows.next()? {
        feed(row.get::<_,&str>(0).as_bytes());feed(row.get::<_,&str>(1).as_bytes());
    }
    Ok(format!("fnv1a64:{hash:016x}"))
}

fn import_transaction(client: &mut Client) -> Result<Transaction<'_>> {
    let mut tx = client.transaction()?;
    // CPU-heavy plan validation can leave an otherwise active import idle.
    // Limit the exemption to this transaction, preserving web-request defaults.
    tx.batch_execute(
        "SET LOCAL idle_in_transaction_session_timeout = '0'; SET LOCAL statement_timeout = '0'",
    )?;
    Ok(tx)
}

fn import_records(
    tx: &mut Transaction<'_>,
    records: &[SourceRecord],
    identities: Option<&[String]>,
    baseline: Option<&crate::dedupe::Snapshot>,
) -> Result<super::ImportDelta> {
    if let (Some(identities), Some(baseline)) = (identities, baseline) {
        return import_bulk::import(tx, records, identities, baseline);
    }
    let mut changed = HashSet::new();
    let mut delta = super::ImportDelta::default();
    let mut progress = identities.map(|_| {
        crate::import_progress::Progress::new("checking/writing source records", records.len())
    });
    // The guarded reconciliation snapshot already contains these rows. Avoid
    // querying PostgreSQL twice per source record just to rediscover them.
    let previous_records: HashMap<_, _> = baseline
        .into_iter()
        .flat_map(|s| &s.sources)
        .map(|(id, record)| {
            (
                (record.source.as_str(), record.external_id.as_str()),
                (id, record),
            )
        })
        .collect();
    let mut known_ids: HashSet<String> = baseline
        .into_iter()
        .flat_map(|s| &s.merchants)
        .map(|m| m.id.clone())
        .collect();
    let previous_query = tx.prepare(
        "SELECT merchant_id,data FROM source_records_documents WHERE source=$1 AND external_id=$2",
    )?;
    let merchant_query = tx.prepare("SELECT id FROM merchants WHERE id=$1")?;
    for (index, record) in records.iter().enumerate() {
        let previous: Option<(String, SourceRecord)> = if baseline.is_some() {
            previous_records
                .get(&(record.source.as_str(), record.external_id.as_str()))
                .map(|(id, old)| ((*id).clone(), (*old).clone()))
        } else {
            tx.query_opt(&previous_query, &[&record.source, &record.external_id])?
                .map(|r| -> Result<(String, SourceRecord)> {
                    Ok((r.get(0), serde_json::from_str(r.get::<_, &str>(1))?))
                })
                .transpose()?
        };
        if let Some((_, old_record)) = &previous {
            let mut old = serde_json::to_value(old_record)?;
            let same_region = crate::markets::dataset_region(old_record)
                == crate::markets::dataset_region(record);
            let mut incoming = serde_json::to_value(record)?;
            incoming["raw"] = record.matching_raw();
            // A new whole-file version does not make unchanged rows a delta.
            // Keep their original import version as provenance.
            old.as_object_mut().unwrap().remove("version");
            incoming.as_object_mut().unwrap().remove("version");
            if same_region && old == incoming {
                delta.unchanged += 1;
                if let Some(progress) = &mut progress {
                    progress.advance(index + 1);
                }
                continue;
            }
            delta.updated += 1;
        } else {
            delta.added += 1;
        }
        let id: String = previous.map(|r| r.0).unwrap_or_else(|| {
            identities
                .map(|ids| ids[index].clone())
                .unwrap_or_else(|| format!("mer_{}", uuid::Uuid::new_v4().simple()))
        });
        let exists = if baseline.is_some() {
            known_ids.contains(&id)
        } else {
            tx.query_opt(&merchant_query, &[&id])?.is_some()
        };
        if !exists {
            let mut seed = record.merchant.clone();
            seed.id = id.clone();
            crate::columns::postgres_merchants(tx, &id, &serde_json::to_string(&seed)?)?;
            known_ids.insert(id.clone());
        }
        crate::columns::postgres_source_records(
            tx,
            &record.source,
            &record.external_id,
            &id,
            &serde_json::to_string(&record)?,
        )?;
        changed.insert(id);
        if let Some(progress) = &mut progress {
            progress.advance(index + 1);
        }
    }
    if let Some(progress) = progress {
        progress.finish();
    }
    let mut progress = identities.map(|_| {
        crate::import_progress::Progress::new("rebuilding merchant search", changed.len())
    });
    let changed: Vec<_> = changed.into_iter().collect();
    for (index, id) in changed.iter().enumerate() {
        rebuild_with_market_refresh(tx, id, false)?;
        if let Some(progress) = &mut progress {
            progress.advance(index + 1);
        }
    }
    if let Some(progress) = progress {
        progress.finish();
    }
    let mut progress = identities.map(|_| {
        crate::import_progress::Progress::new("refreshing merchant markets", changed.len())
    });
    for (index, ids) in changed.chunks(1000).enumerate() {
        refresh_market_lookup(tx, Some(ids))?;
        if let Some(progress) = &mut progress {
            progress.advance(((index + 1) * 1000).min(changed.len()));
        }
    }
    if let Some(progress) = progress {
        progress.finish();
    }
    Ok(delta)
}
fn merge_groups(tx: &mut Transaction<'_>, groups: &[Vec<String>]) -> Result<()> {
    // New source duplicates are written directly to their survivor. There is
    // no persisted identity or outlet reference to update in that case.
    if groups.is_empty() {
        return Ok(());
    }
    let total = groups.iter().map(|g| g.len() - 1).sum();
    let mut progress =
        crate::import_progress::Progress::dedupe("merging duplicate identities", total);
    let mut completed = 0;
    for group in groups {
        let target = &group[0];
        for retired in &group[1..] {
            tx.execute(
                "UPDATE source_records SET merchant_id=$1 WHERE merchant_id=$2",
                &[target, retired],
            )?;
            tx.execute(
                "UPDATE location_records SET merchant_id=$1 WHERE merchant_id=$2",
                &[target, retired],
            )?;
            tx.execute(
                "UPDATE descriptor_resolutions SET merchant_id=$1 WHERE merchant_id=$2",
                &[target, retired],
            )?;
            tx.execute(
                "UPDATE merchant_redirects SET merchant_id=$1 WHERE merchant_id=$2",
                &[target, retired],
            )?;
            tx.execute(
                "INSERT INTO merchant_redirects VALUES($1,$2)",
                &[retired, target],
            )?;
            tx.execute("DELETE FROM manual_merchants WHERE id=$1", &[retired])?;
            rebuild(tx, retired)?;
        }
        rebuild(tx, target)?;
        completed += group.len() - 1;
        progress.advance(completed);
    }
    progress.finish();
    for group in groups { reconcile_locations(tx, Some(&group[0]), false)?; }
    Ok(())
}

fn location_redirects(
    c: &mut impl GenericClient,
) -> Result<std::collections::BTreeMap<String, String>> {
    let exists: bool = c
        .query_one(
            "SELECT to_regclass('public.location_redirects') IS NOT NULL",
            &[],
        )?
        .get(0);
    if !exists {
        return Ok(Default::default());
    }
    Ok(c.query(
        "SELECT retired_id,location_id FROM location_redirects ORDER BY retired_id",
        &[],
    )?
    .into_iter()
    .map(|r| (r.get(0), r.get(1)))
    .collect())
}
fn location_rows(
    c: &mut impl GenericClient,
    merchant_id: Option<&str>,
) -> Result<Vec<(String, LocationRecord)>> {
    c.query("SELECT COALESCE(l.merchant_id,s.merchant_id),l.data FROM location_records_documents l LEFT JOIN source_records s ON s.source=l.merchant_source AND s.external_id=l.merchant_external_id WHERE ($1::text IS NULL OR COALESCE(l.merchant_id,s.merchant_id)=$1) ORDER BY l.id", &[&merchant_id])?.into_iter().map(|r|Ok((r.get(0),serde_json::from_str(r.get::<_,&str>(1))?))).collect()
}
fn reconcile_locations(
    tx: &mut Transaction<'_>,
    merchant_id: Option<&str>,
    dry_run: bool,
) -> Result<crate::location_dedupe::Report> {
    if !dry_run {
        tx.batch_execute(crate::location_dedupe::SCHEMA).context("cannot initialize outlet identity tables; run database init with a schema-owner connection")?;
    }
    if !dry_run {
        tx.execute("DELETE FROM location_redirects WHERE retired_id IN (SELECT d.retired_id FROM location_redirects d JOIN location_records a ON a.id=d.retired_id JOIN location_records b ON b.id=d.location_id LEFT JOIN source_records sa ON sa.source=a.merchant_source AND sa.external_id=a.merchant_external_id LEFT JOIN source_records sb ON sb.source=b.merchant_source AND sb.external_id=b.merchant_external_id WHERE COALESCE(a.merchant_id,sa.merchant_id) IS DISTINCT FROM COALESCE(b.merchant_id,sb.merchant_id))", &[])?;
    }
    let records = location_rows(tx, merchant_id)?;
    let redirects = location_redirects(tx)?;
    let mut report = crate::location_dedupe::plan(&records, &redirects, dry_run);
    if !dry_run && !report.groups.is_empty() {
        let run_id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO location_merge_runs VALUES($1,$2)",
            &[
                &run_id,
                &serde_json::to_string(
                    &serde_json::json!({"report":report,"before":records,"redirects":redirects}),
                )?,
            ],
        )?;
        for group in &report.groups {
            let target = &group[0];
            for retired in &group[1..] {
                tx.execute(
                    "UPDATE location_redirects SET location_id=$1 WHERE location_id=$2",
                    &[target, retired],
                )?;
                tx.execute("INSERT INTO location_redirects VALUES($1,$2) ON CONFLICT(retired_id) DO UPDATE SET location_id=excluded.location_id", &[retired,target])?;
            }
        }
        report.run_id = Some(run_id);
    }
    Ok(report)
}

fn postgres_snapshot_for_ids(
    c: &mut impl GenericClient,
    ids: &[String],
) -> Result<crate::dedupe::Snapshot> {
    fn data<T: serde::de::DeserializeOwned>(
        c: &mut impl GenericClient,
        sql: &str,
        ids: &[String],
    ) -> Result<Vec<T>> {
        c.query(sql, &[&ids])?
            .into_iter()
            .map(|r| Ok(serde_json::from_str(r.get(0))?))
            .collect()
    }
    let source_bytes:i64=c.query_one("SELECT COALESCE(sum(octet_length(data)),0)::bigint FROM source_records_documents WHERE merchant_id=ANY($1)",&[&ids])?.get(0);
    if source_bytes > 64 * 1024 * 1024 {
        bail!(
            "dedupe source evidence exceeds the bounded 64 MiB scan budget; reconcile deterministic identities through source import"
        );
    }
    Ok(crate::dedupe::Snapshot {
        merchants:data(c,"SELECT data FROM merchants_documents WHERE id=ANY($1) ORDER BY id",ids)?,
        manual:data(c,"SELECT data FROM manual_merchants_documents WHERE id=ANY($1) ORDER BY id",ids)?,
        sources:c.query("SELECT merchant_id,data FROM source_records_documents WHERE merchant_id=ANY($1) ORDER BY source,external_id",&[&ids])?.into_iter().map(|r|Ok((r.get(0),serde_json::from_str(r.get(1))?))).collect::<Result<_>>()?,
        locations:data(c,"SELECT l.data FROM location_records_documents l LEFT JOIN source_records s ON s.source=l.merchant_source AND s.external_id=l.merchant_external_id WHERE COALESCE(l.merchant_id,s.merchant_id)=ANY($1) ORDER BY l.id",ids)?,
        redirects:c.query("SELECT retired_id,merchant_id FROM merchant_redirects WHERE merchant_id=ANY($1) ORDER BY retired_id",&[&ids])?.into_iter().map(|r|(r.get(0),r.get(1))).collect(),
        location_redirects:c.query("SELECT d.retired_id,d.location_id FROM location_redirects d JOIN location_records l ON l.id=d.location_id LEFT JOIN source_records s ON s.source=l.merchant_source AND s.external_id=l.merchant_external_id WHERE COALESCE(l.merchant_id,s.merchant_id)=ANY($1) ORDER BY d.retired_id",&[&ids])?.into_iter().map(|r|(r.get(0),r.get(1))).collect(),
        revision:Some(c.query_one("SELECT revision FROM catalog_revision",&[])?.get(0)),
    })
}

#[cfg(test)]
fn postgres_dedupe_snapshot(c: &mut impl GenericClient) -> Result<crate::dedupe::Snapshot> {
    fn data<T: serde::de::DeserializeOwned>(
        c: &mut impl GenericClient,
        sql: &str,
    ) -> Result<Vec<T>> {
        c.query(sql, &[])?
            .into_iter()
            .map(|r| Ok(serde_json::from_str(r.get(0))?))
            .collect()
    }
    let sources = c
        .query(
            "SELECT merchant_id,data FROM source_records_documents ORDER BY source,external_id",
            &[],
        )?
        .into_iter()
        .map(|r| Ok((r.get(0), serde_json::from_str(r.get(1))?)))
        .collect::<Result<_>>()?;
    let redirects_exist: bool = c
        .query_one(
            "SELECT to_regclass('public.merchant_redirects') IS NOT NULL",
            &[],
        )?
        .get(0);
    let redirects = if redirects_exist {
        c.query(
            "SELECT retired_id,merchant_id FROM merchant_redirects ORDER BY retired_id",
            &[],
        )?
        .into_iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect()
    } else {
        vec![]
    };
    Ok(crate::dedupe::Snapshot {
        merchants: data(c, "SELECT data FROM merchants_documents ORDER BY id")?,
        manual: data(c, "SELECT data FROM manual_merchants_documents ORDER BY id")?,
        sources,
        locations: data(c, "SELECT data FROM location_records_documents ORDER BY id")?,
        redirects,
        location_redirects: location_redirects(c)?.into_iter().collect(),
        revision: Some(c.query_one("SELECT revision FROM catalog_revision", &[])?.get(0)),
    })
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
    fn import_transaction_survives_cpu_gaps_and_restores_request_timeouts() -> Result<()> {
        let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
            .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
        let store = PostgresStore::temporary(&base)?;
        store.run(|client| {
            client.batch_execute("SET idle_in_transaction_session_timeout = '1s'")?;
            let mut tx = import_transaction(client)?;
            std::thread::sleep(Duration::from_millis(1500));
            let idle: String = tx
                .query_one("SHOW idle_in_transaction_session_timeout", &[])?
                .get(0);
            let statement: String = tx.query_one("SHOW statement_timeout", &[])?.get(0);
            assert_eq!(idle, "0");
            assert_eq!(statement, "0");
            tx.commit()?;
            let idle: String = client
                .query_one("SHOW idle_in_transaction_session_timeout", &[])?
                .get(0);
            let statement: String = client.query_one("SHOW statement_timeout", &[])?.get(0);
            assert_eq!(idle, "1s");
            assert_eq!(statement, "15s");
            Ok(())
        })
    }

    #[test]
    fn postgres_idle_session_reconnects_before_work_without_replaying_jobs() -> Result<()> {
        let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
            .unwrap_or_else(|_| super::super::LOCAL_DATABASE_URL.into());
        let store = PostgresStore::temporary(&base)?;
        let first_pid: i32 = store.run(|client| {
            let version: i32 = client
                .query_one("SELECT version FROM ultrafinance_schema", &[])?
                .get(0);
            assert_eq!(
                version, 8,
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
