//! Synchronous store on a dedicated worker: postgres owns a Tokio runtime and
//! must never be constructed or dropped on the API/CLI's async runtime threads.
use super::{
    Candidate, Merchant, MerchantPage, RetrievalEvidence, SourceRecord, excluded_aliases,
    normalize, rank_candidates, search_pipeline, validate,
};
use super::{MerchantSourceStats, MerchantStats, STATS_SOURCES, STATS_TOTALS, location_reference};
use crate::location::LocationRecord;
use crate::markets::MarketCountries;
use anyhow::{Context, Result, bail};
use postgres::{Client, GenericClient, IsolationLevel, Transaction};
use postgres_native_tls::MakeTlsConnector;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

#[path = "postgres_store/foursquare_identity.rs"]
mod foursquare_identity;
#[path = "postgres_store/foursquare_locations.rs"]
mod foursquare_locations;
#[path = "postgres_store/import_bulk.rs"]
mod import_bulk;
#[path = "postgres_store/indexed_import.rs"]
mod indexed_import;
#[path = "postgres_store/initial_load.rs"]
mod initial_load;
#[path = "postgres_store/logos.rs"]
mod logos;
#[path = "postgres_store/merchant_provenance.rs"]
mod merchant_provenance;
#[path = "postgres_store/numeric_ids.rs"]
mod numeric_ids;
#[path = "postgres_store/read_cache.rs"]
mod read_cache;
#[cfg(test)]
#[path = "../tests/unit/postgres_store/read_cache_tests.rs"]
mod read_cache_tests;
#[path = "postgres_store/research.rs"]
mod research;
#[path = "postgres_store/rows.rs"]
pub(super) mod rows;
#[path = "postgres_store/typed_sources.rs"]
mod typed_sources;

type Job = Box<dyn FnOnce(Result<&mut Client>) + Send>;
#[derive(Clone)]
pub(super) struct PostgresStore(Arc<Worker>);
struct Worker {
    reads: Mutex<read_cache::ReadCache>,
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
            let row = tx.query_opt("SELECT * FROM merchants WHERE id=$1", &[&id])?;
            row.map(|r| rows::merchant(&r)).transpose()
        })
    }
    pub fn resolutions(
        &self,
        id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::resolution::Resolution>> {
        let id = id.map(str::to_owned);
        self.run(move |c| {
            c.query(
                &format!(
                    "{} WHERE ($1::text IS NULL OR r.id=$1) ORDER BY r.id LIMIT $2",
                    rows::RESOLUTIONS
                ),
                &[&id, &(limit as i64)],
            )?
            .iter()
            .map(rows::resolution)
            .collect()
        })
    }
    pub fn save_resolution(&self, resolution: &crate::resolution::Resolution) -> Result<bool> {
        let resolution = resolution.clone();
        self.run(move |c| {
            let mut tx = c.transaction()?;
            write_lock(&mut tx)?;
            let old = tx
                .query_opt(
                    &format!("{} WHERE r.id=$1", rows::RESOLUTIONS),
                    &[&resolution.id],
                )?
                .map(|r| rows::resolution(&r))
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
                    // Schema upgrades may wait for existing API reads to finish.
                    // Keep the shorter runtime lock timeout outside this transaction.
                    tx.batch_execute("SET LOCAL lock_timeout='60s'; SET LOCAL statement_timeout='60s'")?;
                    write_lock(&mut tx).context("schema upgrade could not acquire the catalog writer lock")?;
                    let exists: bool = tx.query_one("SELECT to_regclass('public.ultrafinance_schema') IS NOT NULL", &[])?.get(0);
                    if !exists { tx.batch_execute(include_str!("../migrations/001_postgres.sql"))?; }
                    let version: i32 = tx.query_one("SELECT version FROM ultrafinance_schema", &[])?.get(0);
                    if version == 1 { tx.batch_execute(include_str!("../migrations/002_enrichment_log.sql"))?; }
                    if version <= 2 { tx.batch_execute(include_str!("../migrations/003_locations.sql"))?; }
                    if version <= 2 { migrate_markets(&mut tx)?; }
                    tx.batch_execute(crate::dedupe::SCHEMA)
                        .context("schema upgrade could not initialize merchant redirects")?;
                    tx.batch_execute(crate::location_dedupe::SCHEMA)
                        .context("schema upgrade could not initialize location redirects")?;
                    if version > super::POSTGRES_SCHEMA_VERSION {
                        return Err(super::SchemaMismatch { found: version, required: super::POSTGRES_SCHEMA_VERSION }.into());
                    }
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
                    }
                    if version < 8 {
                        tx.batch_execute("SET LOCAL statement_timeout='0'; SET LOCAL idle_in_transaction_session_timeout='0'")?;
                        tx.batch_execute("UPDATE ultrafinance_schema SET version=8")?;
                    }
                    if version < 9 {
                        tx.batch_execute(include_str!("../migrations/009_remove_market_evidence.sql"))?;
                    }
                    if version < 10 {
                        tx.batch_execute(include_str!("../migrations/010_remove_merge_audits.sql"))?;
                    }
                    if version < 11 {
                        tx.batch_execute("SET LOCAL statement_timeout='0'; SET LOCAL idle_in_transaction_session_timeout='0'")?;
                        tx.batch_execute(include_str!("../migrations/011_search_words.sql"))?;
                    }
                    if version < 12 {
                        tx.batch_execute("SET LOCAL statement_timeout='0'; SET LOCAL idle_in_transaction_session_timeout='0'")?;
                        tx.batch_execute(include_str!("../migrations/012_import_host_index.sql"))
                            .context("schema migration 12 (merchant host index) failed")?;
                    }
                    if version < 13 {
                        // Bound hash-join spills during the large outlet backfill.
                        tx.batch_execute("SET LOCAL statement_timeout='0'; SET LOCAL idle_in_transaction_session_timeout='0'; SET LOCAL work_mem='64MB'")?;
                        tx.batch_execute(include_str!("../migrations/013_location_ownership.sql"))
                            .context("schema migration 13 (outlet ownership) failed")?;
                    }
                    if version < 14 {
                        tx.batch_execute(include_str!("../migrations/014_remove_document_views.sql"))
                            .context("schema migration 14 (document views) failed")?;
                    }
                    if version < 15 {
                        tx.batch_execute("SET LOCAL statement_timeout='0'; SET LOCAL idle_in_transaction_session_timeout='0'")?;
                        tx.batch_execute(include_str!("../migrations/015_typed_catalog_lists.sql"))
                            .context("schema migration 15 (typed catalog lists) failed")?;
                    }
                    if version < 16 {
                        tx.batch_execute(include_str!("../migrations/016_remove_unused_indexes.sql"))
                            .context("schema migration 16 (unused search indexes) failed")?;
                    }
                    if version < 17 {
                        tx.batch_execute("SET LOCAL statement_timeout='0'; SET LOCAL idle_in_transaction_session_timeout='0'; SET LOCAL work_mem='64MB'")?;
                        tx.batch_execute(include_str!("../migrations/017_source_provenance.sql"))
                            .context("schema migration 17 (shared source provenance) failed")?;
                    }
                    if version < 18 {
                        tx.batch_execute(include_str!("../migrations/018_foursquare_identity_only.sql"))
                            .context("schema migration 18 (Foursquare import identities) failed")?;
                    }
                    if version < 19 {
                        tx.batch_execute("SET LOCAL statement_timeout='0'; SET LOCAL idle_in_transaction_session_timeout='0'; SET LOCAL work_mem='64MB'")?;
                        tx.batch_execute(include_str!("../migrations/019_numeric_merchant_keys.sql"))
                            .context("schema migration 19 (numeric merchant keys) failed")?;
                    }
                    if version < 4 { refresh_market_lookup(&mut tx,None)?; }
                    // Run historical normalizer backfills after key conversion.
                    if version < 12 { indexed_import::backfill(&mut tx)?; }
                    tx.batch_execute(include_str!("../schema/logos.sql"))
                        .context("schema initialization could not initialize website logos")?;
                    tx.commit()?;
                }
                let version: i32 = client.query_one("SELECT version FROM ultrafinance_schema", &[])
                    .context("PostgreSQL schema missing; run `ultrafinance database init` first")?.get(0);
                if version != super::POSTGRES_SCHEMA_VERSION {
                    return Err(super::SchemaMismatch { found: version, required: super::POSTGRES_SCHEMA_VERSION }.into());
                }
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
            reads: Mutex::new(read_cache::ReadCache::default()),
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
    pub fn write_logs(
        &self,
        entries: Vec<(String, String, String, Option<String>, String)>,
    ) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        self.run(move |client| {
            let ids: Vec<_> = entries.iter().map(|r| &r.0).collect();
            let batches: Vec<_> = entries.iter().map(|r| &r.1).collect();
            let statuses: Vec<_> = entries.iter().map(|r| &r.2).collect();
            let merchants: Vec<_> = entries.iter().map(|r| &r.3).collect();
            let documents: Vec<_> = entries.iter().map(|r| &r.4).collect();
            client.execute("INSERT INTO enrichment_log(id,batch_id,status,merchant_id,data,finished_at) SELECT id,batch_id,status,merchant_id,data,CASE WHEN status='started' THEN NULL ELSE clock_timestamp() END FROM unnest($1::text[],$2::text[],$3::text[],$4::text[],$5::text[]) AS rows(id,batch_id,status,merchant_id,data) ON CONFLICT(id) DO UPDATE SET status=excluded.status,merchant_id=excluded.merchant_id,data=excluded.data,finished_at=excluded.finished_at", &[&ids,&batches,&statuses,&merchants,&documents])?;
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
                let overridden: bool = tx.query_opt("SELECT manual_override FROM location_records WHERE source=$1 AND external_id=$2", &[&record.source,&record.external_id])?.is_some_and(|r| r.get(0));
                if !record.manual_override && overridden { continue; }
                if let crate::location::MerchantReference::Source { source,external_id } = &record.merchant
                    && source == "foursquare" {
                    let merchant_id = tx.query_one("SELECT merchant_id FROM source_identities WHERE source='foursquare' AND external_id=$1", &[external_id])?.get(0);
                    record.merchant = crate::location::MerchantReference::Local { merchant_id };
                }
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
                if record.source == "foursquare" { tx.execute("DELETE FROM foursquare_unlocated WHERE external_id=$1", &[&record.external_id])?; }

            }
            reconcile_locations(&mut tx, None, false)?;
            refresh_market_lookup(&mut tx, None)?;
            tx.commit()?;
            Ok(())
        })
    }
    pub fn locations(&self, merchant_id: &str) -> Result<Vec<LocationRecord>> {
        Ok(self.cached_locations(merchant_id)?.as_ref().clone())
    }
    pub fn cached_locations(&self, merchant_id: &str) -> Result<Arc<Vec<LocationRecord>>> {
        let merchant_id = merchant_id.to_owned();
        let owner = self.0.clone();
        self.run(move |client| {
            let mut tx = client
                .build_transaction()
                .isolation_level(IsolationLevel::RepeatableRead)
                .read_only(true)
                .start()?;
            let output = owner.reads.lock().unwrap().outlets(&mut tx, &merchant_id)?;
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
            if source == "foursquare" {
                return client.query("SELECT external_id,merchant_id FROM source_identities WHERE source='foursquare' AND ($1::text IS NULL OR external_id=$1) ORDER BY external_id LIMIT $2 OFFSET $3", &[&external_id,&limit,&offset])?.iter().map(|r| Ok(serde_json::json!({"merchant_id":r.get::<_,String>("merchant_id"),"record":{"source":"foursquare","external_id":r.get::<_,String>("external_id")},"identity_only":true}))).collect();
            }
            client.query("SELECT s.*,p.attribution,p.license,p.url FROM source_records s JOIN source_provenance p ON p.id=s.source_provenance_id WHERE s.source=$1 AND ($2::text IS NULL OR external_id=$2) ORDER BY external_id LIMIT $3 OFFSET $4", &[&source,&external_id,&limit,&offset])?.into_iter().map(|row| {
                let record = rows::source(&row)?;
                let mut document = serde_json::to_value(&record)?;
                // Source listings retain explicit null/empty input fields.
                let merchant = &record.merchant;
                document["merchant"] = serde_json::json!({
                    "id":merchant.id,"name":merchant.name,"website":merchant.website,
                    "logo_url":merchant.logo_url,"logo_source":merchant.logo_source,
                    "markets":merchant.markets,"aliases":merchant.aliases,"sources":merchant.sources
                });
                Ok(serde_json::json!({"merchant_id": row.try_get::<_,String>("merchant_id")?, "record": document}))
            }).collect()
        })
    }
    pub fn merchant_provenance(&self, id: &str) -> Result<Vec<SourceRecord>> {
        let id = id.to_owned();
        self.run(move |c| {
            Ok(merchant_provenance::read(c, std::slice::from_ref(&id))?
                .remove(&id)
                .unwrap_or_default())
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
                    "SELECT merchant_id FROM source_identities WHERE source=$1 AND external_id=$2",
                    &[&source, &external_id],
                )?
                .get(0);
            if tx
                .query_opt("SELECT id FROM merchants WHERE id=$1", &[&target])?
                .is_none()
            {
                bail!("target merchant does not exist");
            }
            if source == "foursquare" {
                tx.execute("UPDATE location_records SET merchant_id=merchant_key($2) WHERE source='foursquare' AND external_id=$1", &[&external_id,&target])?;
                tx.execute("UPDATE foursquare_unlocated SET merchant_id=merchant_key($2) WHERE external_id=$1", &[&external_id,&target])?;
            } else {
            tx.execute(
                "UPDATE source_records SET merchant_id=$3 WHERE source=$1 AND external_id=$2",
                &[&source, &external_id, &target],
            )?;
            }
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
                "SELECT merchant_id FROM source_identities WHERE source=$1 AND external_id=$2",
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
SELECT merchant_public_id(c.left_id),merchant_public_id(c.right_id),c.exact,a.normalized_name,b.normalized_name FROM candidates c JOIN merchant_identity_keys a ON a.merchant_id=c.left_id JOIN merchant_identity_keys b ON b.merchant_id=c.right_id"#,std::iter::empty::<&str>())?;
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
    ) -> Result<String> {
        let (expected, groups) = (expected.clone(), groups.to_vec());
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
            merge_groups(&mut tx, &groups)?;
            tx.commit()?;
            Ok(run_id)
        })
    }
    pub fn apply_dedupe_events(&self, revision: i64, groups: Vec<Vec<String>>) -> Result<String> {
        self.run(move |client| {
            let mut tx = import_transaction(client)?;
            write_lock(&mut tx)?;
            tx.batch_execute("LOCK TABLE merchants,manual_merchants,source_records,location_records,descriptor_resolutions,merchant_redirects IN SHARE ROW EXCLUSIVE MODE")?;
            let current: i64 = tx.query_one("SELECT revision FROM catalog_revision", &[])?.get(0);
            if current != revision {
                bail!("catalog changed since preview; regenerate the merge plan");
            }
            for group in groups {
                let unique: std::collections::BTreeSet<_> = group.iter().collect();
                if group.len() < 2 || unique.len() != group.len() {
                    bail!("merge groups must contain at least two distinct merchant IDs");
                }
                let existing: i64 = tx.query_one("SELECT count(*) FROM merchants WHERE id=ANY($1)", &[&group])?.get(0);
                if existing as usize != group.len() {
                    bail!("merge plan references a missing or already retired merchant");
                }
                let manuals: i64 = tx.query_one("SELECT count(*) FROM manual_merchants WHERE id=ANY($1)", &[&group])?.get(0);
                if manuals > 1 { bail!("merge plan contains conflicting manual identities"); }
                merge_groups(&mut tx, &[group])?;
            }
            tx.commit()?;
            Ok(uuid::Uuid::new_v4().to_string())
        })
    }
    pub fn reconcile_import(
        &self,
        records: Vec<SourceRecord>,
        dry_run: bool,
        chunk_size: usize,
    ) -> Result<crate::dedupe::ImportReport> {
        if chunk_size == 0 {
            bail!("import chunk size must be greater than zero");
        }
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
    pub(super) fn import_database_identity(&self) -> Result<String> {
        self.run(|client| {
            let row = client.query_one("SELECT current_database(),oid::text FROM pg_database WHERE datname=current_database()", &[])?;
            Ok(crate::eval::fingerprint(format!("{}:{}", row.get::<_, String>(0), row.get::<_, String>(1)).as_bytes()))
        })
    }
    pub(super) fn reconcile_generated<F>(
        &self,
        total: usize,
        dry_run: bool,
        chunk_size: usize,
        produce: F,
    ) -> Result<crate::dedupe::ImportReport>
    where
        F: FnOnce(&mut dyn FnMut(Vec<SourceRecord>) -> Result<()>) -> Result<()> + Send + 'static,
    {
        if chunk_size == 0 {
            bail!("import chunk size must be greater than zero");
        }
        self.run(move |client| {
            let _quiet = (!dry_run && !crate::import_progress::verbose()).then(crate::import_progress::quiet);
            let mut tx = import_transaction(client)?;
            write_lock(&mut tx)?;
            tx.batch_execute("LOCK TABLE merchants,manual_merchants,source_records,location_records,descriptor_resolutions,merchant_redirects IN SHARE ROW EXCLUSIVE MODE")?;
            // Counting the entire catalog once per committed chunk would be quadratic.
            let mut importer = indexed_import::Import::new_stream(&mut tx, dry_run, total, chunk_size)?;
            produce(&mut |chunk| importer.chunk(chunk))?;
            let report = importer.finish()?;
            if dry_run { tx.rollback()?; } else { tx.commit()?; }
            Ok(report)
        })
    }
    pub(super) fn initial_foursquare_load<F>(
        &self,
        total: usize,
        dry_run: bool,
        chunk_size: usize,
        produce: F,
    ) -> Result<crate::dedupe::ImportReport>
    where
        F: FnOnce(&mut dyn FnMut(Vec<SourceRecord>) -> Result<()>) -> Result<()> + Send + 'static,
    {
        self.run(move |client| initial_load::run(client, total, dry_run, chunk_size, produce))
    }
    pub fn reconcile_file(
        &self,
        path: std::path::PathBuf,
        limit: Option<u32>,
        dry_run: bool,
        chunk_size: usize,
    ) -> Result<(crate::dedupe::ImportReport, usize, usize)> {
        if chunk_size == 0 {
            bail!("import chunk size must be greater than zero");
        }
        // Counting a global bundle can take many minutes. Do this before
        // dispatching the database job so its health probe reconnects any
        // connection that expired while we were reading the file.
        let total = crate::datasets::count_records(&path)?;
        let selected_total = limit.map_or(total, |n| total.min(n as usize));
        eprintln!("Import: {total} available source records; {selected_total} selected");
        eprintln!("Import: waiting for database write lock");
        self.run(move |client| {
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
                "SELECT count(*) FROM merchants m WHERE cardinality(m.markets)=0",
                &[],
            )?.get::<_, i64>(0))?;
            let by_market = tx.query(
                "SELECT country,count(DISTINCT m.id) FROM merchants m CROSS JOIN LATERAL unnest(m.markets) country GROUP BY country ORDER BY count(DISTINCT m.id) DESC,country",
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
            let total: i64 = tx.query_one("SELECT COUNT(*) FROM merchants m WHERE ($1::text IS NULL OR m.markets @> ARRAY[$1::text])", &[&market])?.get(0);
            let mut merchants = Vec::new();
            for row in tx.query("SELECT m.* FROM merchants m WHERE ($1::text IS NULL OR m.markets @> ARRAY[$1::text]) ORDER BY lower(name) COLLATE \"C\",id LIMIT $2 OFFSET $3", &[&market,&(limit as i64),&sql_offset])? {
                merchants.push(rows::merchant(&row)?);
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
        let owner = self.0.clone();
        self.run(move |client| {
            let mut tx = client.build_transaction().isolation_level(IsolationLevel::RepeatableRead).read_only(true).start()?;
            // LIMIT does not bound a sequential scan for an absent term. Prefer
            // the existing alias/GIN indexes for these small, independent pools.
            tx.batch_execute("SET LOCAL enable_seqscan=off")?;
            let mut found = HashMap::new();
            let mut scorer = crate::search_score::Scorer::new(&query);
            let exact = tx.query("SELECT m.id,m.name,m.aliases,m.markets FROM merchants m JOIN aliases a ON a.merchant_id=m.merchant_key WHERE a.normalized=$1 ORDER BY m.id LIMIT 255", &[&query])?;
            for row in exact {
                add_candidate(&mut found, &row, true, None, &mut scorer, ("exact_alias", &query))?;
            }
            let mut matches: HashMap<String, usize> = HashMap::new();
            let rules = owner.reads.lock().unwrap().rules(&mut tx)?;
            for (id, pattern) in rules.iter() {
                if let Some(length) = pattern.match_length(&description) {
                    matches.entry(id.clone()).and_modify(|v| *v = (*v).max(length)).or_insert(length);
                }
            }
            for (id, length) in matches {
                if !found.contains_key(&id) {
                    let row = tx.query_one("SELECT id,name,aliases,markets FROM merchants WHERE id=$1", &[&id])?;
                    add_candidate(&mut found, &row, false, Some(length), &mut scorer, ("source_regex", &query))?;
                }
                if let Some(candidate) = found.get_mut(&id) { candidate.regex_match_length = Some(length); }
            }
            // Alias spans have their own indexed budget, independent of common
            // full-text posting lists. A span is retrieval evidence, never trust.
            let spans = search_pipeline::alias_spans(&query);
            if !spans.is_empty() {
                for row in tx.query(
                    "SELECT m.id,m.name,m.aliases,m.markets,span.text FROM unnest($1::text[]) span(text)
                     CROSS JOIN LATERAL (
                         SELECT merchant_id FROM aliases WHERE normalized=span.text
                         ORDER BY merchant_id LIMIT 64
                     ) a JOIN merchants m ON m.merchant_key=a.merchant_id",
                    &[&spans],
                )? {
                    add_candidate(&mut found, &row, false, None, &mut scorer,
                        ("alias_span", row.get::<_, &str>(4)))?;
                }
            }
            let tokens: Vec<_> = query.split_whitespace().filter(|t| t.chars().count()>=3).take(16).collect();
            let terms: Vec<_> = tokens.iter().map(|t| format!("'{t}'")).collect();
            // Two common words can each fill their independent pool before a
            // business containing both appears. Intersect adjacent words in
            // the index as an additional bounded pool, retaining the original
            // single-word queries for abbreviations and noisy descriptors.
            let phrase_tokens: Vec<_> = query.split_whitespace().take(16).collect();
            let mut combined_terms: Vec<_> = phrase_tokens.windows(2).chain(tokens.windows(2))
                .filter(|pair| pair[0] != pair[1] && pair.iter().any(|t| t.chars().count() >= 3))
                .map(|pair| format!("'{}' & '{}'", pair[0], pair[1]))
                .collect();
            combined_terms.sort_unstable();
            combined_terms.dedup();
            if !combined_terms.is_empty() {
                for row in tx.query(
                    "SELECT m.id,m.name,m.aliases,m.markets FROM (
                        SELECT DISTINCT pool.merchant_id
                        FROM unnest($1::text[]) term(query)
                        CROSS JOIN LATERAL (
                            SELECT merchant_id FROM merchant_search
                            WHERE tokens @@ to_tsquery('simple',term.query)
                            LIMIT 32
                        ) pool
                    ) s JOIN merchants m ON m.merchant_key=s.merchant_id",
                    &[&combined_terms],
                )? {
                    add_candidate(&mut found,&row,false,None,&mut scorer,("token_pair", &query))?;
                }
            }
            // Bound each word independently: common words must not crowd rare
            // ones out before Rust scores merchant names. A post-sort LIMIT alone
            // does not bound work on a million-row catalog.
            if !terms.is_empty() {
                for row in tx.query(
                    "SELECT m.id,m.name,m.aliases,m.markets FROM (
                        SELECT DISTINCT pool.merchant_id
                        FROM unnest($1::text[]) term(query)
                        CROSS JOIN LATERAL (
                            SELECT merchant_id FROM merchant_search
                            WHERE tokens @@ to_tsquery('simple',term.query)
                            LIMIT 100
                        ) pool
                    ) s JOIN merchants m ON m.merchant_key=s.merchant_id",
                    &[&terms],
                )? {
                    add_candidate(&mut found,&row,false,None,&mut scorer,("token", &query))?;
                }
            }
            // Fuzzy-match the much smaller word vocabulary, then retrieve IDs
            // through the full-text index. Each word contributes a bounded pool.
            // Store numbers and dates are not spelling evidence for a brand.
            let fuzzy_tokens: Vec<_> = tokens.iter().copied()
                .filter(|token| token.chars().any(char::is_alphabetic)).collect();
            let fuzzy_words = owner.reads.lock().unwrap().fuzzy_words(&mut tx, &fuzzy_tokens)?;
            if !fuzzy_words.is_empty() {
                for row in tx.query(
                    "SELECT m.id,m.name,m.aliases,m.markets FROM (
                        SELECT DISTINCT pool.merchant_id
                        FROM unnest($1::text[]) words(word)
                        CROSS JOIN LATERAL (
                            SELECT merchant_id FROM merchant_search
                            WHERE tokens @@ plainto_tsquery('simple',words.word)
                            LIMIT 32
                        ) pool
                    ) s JOIN merchants m ON m.merchant_key=s.merchant_id",
                    &[&fuzzy_words],
                )? {
                    add_candidate(&mut found,&row,false,None,&mut scorer,("fuzzy_token", &query))?;
                }
            }
            // Trust and negative aliases affect eligibility/ranking. Read only
            // those fields for the retrieval pool, then hydrate the shortlist.
            let ids: Vec<_> = found.keys().cloned().collect();
            for row in tx.query("SELECT * FROM manual_merchants WHERE id=ANY($1)", &[&ids])? {
                let manual = rows::merchant(&row)?;
                if let Some(candidate) = found.get_mut(&manual.id) {
                    candidate.trusted = std::iter::once(&manual.name).chain(&manual.aliases)
                        .any(|name| normalize(name) == query);
                }
            }
            for row in tx.query("SELECT merchant_id,negative_aliases FROM source_records WHERE merchant_id=ANY($1) AND cardinality(negative_aliases)>0", &[&ids])? {
                let id: String = row.get(0);
                let aliases: Vec<String> = row.get(1);
                if found.get(&id).is_some_and(|c| !c.trusted)
                    && excluded_aliases(&query, aliases.iter().map(String::as_str)) {
                    found.remove(&id);
                }
            }
            let mut shortlisted = rank_candidates(found, limit, country.as_deref());
            let ids: Vec<_> = shortlisted.iter().map(|c| c.merchant.id.clone()).collect();
            let mut merchants: HashMap<String, Merchant> = tx
                .query("SELECT * FROM merchants WHERE id=ANY($1)", &[&ids])?
                .into_iter()
                .map(|row| Ok((row.get("id"), rows::merchant(&row)?)))
                .collect::<Result<_>>()?;
            let mut sources = merchant_provenance::read(&mut tx, &ids)?;
            for candidate in &mut shortlisted {
                candidate.merchant = merchants.remove(&candidate.merchant.id)
                    .context("shortlisted merchant disappeared within its snapshot")?;
                candidate.provenance = sources.remove(&candidate.merchant.id).unwrap_or_default();
            }
            tx.commit()?;
            Ok(shortlisted)
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
fn postgres_markets(
    tx: &mut impl GenericClient,
    ids: Option<&[String]>,
) -> Result<MarketCountries> {
    let mut index = MarketCountries::default();
    let filter = |column: &str| {
        if ids.is_some() {
            format!("{column} = ANY($1::text[])")
        } else {
            "$1::text[] IS NULL".into()
        }
    };
    let manual_sql = format!("SELECT * FROM manual_merchants WHERE {}", filter("id"));
    for row in tx.query(&manual_sql, &[&ids])? {
        index.declaration(row.get("id"), &rows::merchant(&row)?);
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
        "SELECT DISTINCT merchant_public_id(merchant_id),country FROM location_records WHERE merchant_id=ANY(merchant_keys($1)) AND country IS NOT NULL"
    } else {
        "SELECT DISTINCT merchant_public_id(merchant_id),country FROM location_records WHERE $1::text[] IS NULL AND country IS NOT NULL"
    };
    let mut rows = tx.query_raw(sql, [&ids as &(dyn postgres::types::ToSql + Sync)])?;
    while let Some(row) = rows.next()? {
        index.outlet_country(row.get(0), row.get(1));
    }
    Ok(())
}

fn add_candidate(
    found: &mut HashMap<String, Candidate>,
    row: &postgres::Row,
    exact: bool,
    regex_match_length: Option<usize>,
    scorer: &mut crate::search_score::Scorer<'_>,
    retrieval: (&'static str, &str),
) -> Result<()> {
    // Rank with just identity, names and markets; hydrate the final shortlist.
    let merchant = Merchant {
        id: row.get(0),
        name: row.get(1),
        aliases: row.try_get(2)?,
        markets: row.try_get(3)?,
        website: None,
        logo_url: None,
        logo_source: None,
        sources: vec![],
    };
    if let Some(existing) = found.get_mut(&merchant.id) {
        existing.retrieval_evidence.push(RetrievalEvidence {
            path: retrieval.0,
            query: retrieval.1.into(),
            score: existing.score,
        });
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
    found.insert(
        merchant.id.clone(),
        Candidate {
            resolution_id: None,
            pending_import: false,
            interpretation_evidence: vec![],
            retrieval_evidence: vec![RetrievalEvidence {
                path: retrieval.0,
                query: retrieval.1.into(),
                score,
            }],
            merchant,
            score,
            exact,
            regex_match_length,
            trusted: false,
            provenance: vec![],
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
        .query_opt("SELECT * FROM manual_merchants WHERE id=$1", &[&id])?
        .map(|r| rows::merchant(&r))
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
    tx.execute(
        "DELETE FROM aliases WHERE merchant_id=merchant_key($1)",
        &[&id],
    )?;
    tx.execute(
        "DELETE FROM merchant_search WHERE merchant_id=merchant_key($1)",
        &[&id],
    )?;
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
        tx.execute(
            "INSERT INTO aliases VALUES(merchant_key($1),$2)",
            &[&id, name],
        )?;
    }
    tx.execute(
        "INSERT INTO merchant_search(merchant_id,text) VALUES(merchant_key($1),$2)",
        &[&id, &names.join(" \n ")],
    )?;
    if refresh_markets {
        refresh_market_lookup(tx, Some(&[id.to_owned()]))?;
    }
    Ok(())
}

fn refresh_market_lookup(c: &mut impl GenericClient, ids: Option<&[String]>) -> Result<()> {
    if ids.is_none() {
        let mut after = String::new();
        loop {
            let batch: Vec<String> = c
                .query(
                    "SELECT id FROM merchants WHERE id>$1 ORDER BY id LIMIT 1000",
                    &[&after],
                )?
                .into_iter()
                .map(|r| r.get(0))
                .collect();
            if batch.is_empty() {
                return Ok(());
            }
            after = batch.last().unwrap().clone();
            refresh_market_lookup(c, Some(&batch))?;
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
        if let Some(row) = c.query_opt("SELECT * FROM merchants WHERE id=$1", &[&id])? {
            let merchant = rows::merchant(&row)?;
            let markets = index.hydrate(merchant).markets;
            c.execute(
                "UPDATE merchants SET markets=$2 WHERE id=$1 AND markets IS DISTINCT FROM $2",
                &[&id, &markets],
            )?;
        }
    }

    Ok(())
}

fn import_transaction(client: &mut Client) -> Result<Transaction<'_>> {
    let mut tx = client.transaction()?;
    // CPU-heavy plan validation can leave an otherwise active import idle.
    // Bounded bulk lookups/sorts also need more than the default 4MB to avoid
    // spilling to disk. Keep these settings local to the import transaction.
    tx.batch_execute(
        "SET LOCAL idle_in_transaction_session_timeout = '0'; SET LOCAL statement_timeout = '0'; SET LOCAL work_mem = '64MB'",
    )?;
    Ok(tx)
}

fn import_records(
    tx: &mut Transaction<'_>,
    records: &[SourceRecord],
    identities: Option<&[String]>,
    baseline: Option<&crate::dedupe::Snapshot>,
) -> Result<super::ImportDelta> {
    if records.iter().any(|r| r.source == "foursquare") {
        let (fsq, other): (Vec<_>, Vec<_>) = records
            .iter()
            .cloned()
            .partition(|r| r.source == "foursquare");
        let (mut delta, _) = foursquare_identity::import(tx, &fsq)?;
        if !other.is_empty() {
            let rest = import_records(tx, &other, None, None)?;
            delta.added += rest.added;
            delta.updated += rest.updated;
            delta.unchanged += rest.unchanged;
        }
        return Ok(delta);
    }
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
    let previous_query =
        tx.prepare("SELECT s.*,p.attribution,p.license,p.url FROM source_records s JOIN source_provenance p ON p.id=s.source_provenance_id WHERE s.source=$1 AND external_id=$2")?;
    let merchant_query = tx.prepare("SELECT id FROM merchants WHERE id=$1")?;
    for (index, record) in records.iter().enumerate() {
        let previous: Option<(String, SourceRecord)> = if baseline.is_some() {
            previous_records
                .get(&(record.source.as_str(), record.external_id.as_str()))
                .map(|(id, old)| ((*id).clone(), (*old).clone()))
        } else {
            tx.query_opt(&previous_query, &[&record.source, &record.external_id])?
                .map(|r| -> Result<(String, SourceRecord)> {
                    Ok((r.get("merchant_id"), rows::source(&r)?))
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
            let fsq: bool = tx.query_one("SELECT EXISTS(SELECT 1 FROM source_identities WHERE source='foursquare' AND merchant_id=$1)", &[retired])?.get(0);
            if fsq {
                let mut target_merchant = rows::merchant(
                    &tx.query_one("SELECT * FROM merchants WHERE id=$1", &[target])?,
                )?;
                let retired_merchant = rows::merchant(
                    &tx.query_one("SELECT * FROM merchants WHERE id=$1", &[retired])?,
                )?;
                crate::dedupe::combine(&mut target_merchant, &retired_merchant);
                import_bulk::write_current_merchants(tx, &[target_merchant])?;
            }
            tx.execute(
                "UPDATE foursquare_unlocated SET merchant_id=merchant_key($1) WHERE merchant_id=merchant_key($2)",
                &[target, retired],
            )?;
            tx.execute(
                "UPDATE source_records SET merchant_id=$1 WHERE merchant_id=$2",
                &[target, retired],
            )?;
            tx.execute(
                "UPDATE location_records SET merchant_id=merchant_key($1) WHERE merchant_id=merchant_key($2)",
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
    for group in groups {
        reconcile_locations(tx, Some(&group[0]), false)?;
    }
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
    // Canonical merchant IDs use the merchant index directly.
    let filter = if merchant_id.is_some() {
        "l.merchant_id=merchant_key($1)"
    } else {
        "$1::text IS NULL"
    };
    c.query(
        &format!("{} WHERE {filter} ORDER BY l.id", rows::LOCATIONS),
        &[&merchant_id],
    )?
    .iter()
    .map(|r| Ok((r.try_get("public_merchant_id")?, rows::location(r)?)))
    .collect()
}
fn reconcile_locations(
    tx: &mut Transaction<'_>,
    merchant_id: Option<&str>,
    dry_run: bool,
) -> Result<crate::location_dedupe::Report> {
    if !dry_run {
        tx.batch_execute(crate::location_dedupe::SCHEMA).context("cannot initialize outlet identity tables; run database init with a schema-owner connection")?;
    }
    let removed = if !dry_run {
        tx.execute("DELETE FROM location_redirects WHERE retired_id IN (SELECT d.retired_id FROM location_redirects d JOIN location_records a ON a.id=d.retired_id JOIN location_records b ON b.id=d.location_id WHERE a.merchant_id IS DISTINCT FROM b.merchant_id)", &[])?
    } else {
        0
    };
    let records = location_rows(tx, merchant_id)?;
    let redirects = location_redirects(tx)?;
    let mut report = crate::location_dedupe::plan(&records, &redirects, dry_run);
    if !dry_run && !report.groups.is_empty() {
        let run_id = uuid::Uuid::new_v4().to_string();
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
    if !dry_run && (removed > 0 || !report.groups.is_empty()) {
        // Redirect-only dedupe does not touch a table with a revision trigger.
        // Other connections must still invalidate their cached outlet identities.
        tx.execute("UPDATE catalog_revision SET revision=revision+1", &[])?;
    }
    Ok(report)
}

fn postgres_snapshot_for_ids(
    c: &mut impl GenericClient,
    ids: &[String],
) -> Result<crate::dedupe::Snapshot> {
    postgres_snapshot_for_ids_inner(c, ids, true)
}

fn postgres_snapshot_for_ids_inner(
    c: &mut impl GenericClient,
    ids: &[String],
    include_locations: bool,
) -> Result<crate::dedupe::Snapshot> {
    let merchants = c
        .query(
            "SELECT * FROM merchants WHERE id=ANY($1) ORDER BY id",
            &[&ids],
        )?
        .iter()
        .map(rows::merchant)
        .collect::<Result<_>>()?;
    let manual = c
        .query(
            "SELECT * FROM manual_merchants WHERE id=ANY($1) ORDER BY id",
            &[&ids],
        )?
        .iter()
        .map(rows::merchant)
        .collect::<Result<_>>()?;
    let sources = rows::bounded_sources(c, ids)?;
    let locations = if include_locations {
        c.query(
            &format!(
                "{} WHERE l.merchant_id=ANY(merchant_keys($1)) ORDER BY l.id",
                rows::LOCATIONS
            ),
            &[&ids],
        )?
        .iter()
        .map(rows::location)
        .collect::<Result<_>>()?
    } else {
        vec![]
    };
    Ok(crate::dedupe::Snapshot {
        merchants, manual, sources, locations,
        redirects:c.query("SELECT retired_id,merchant_id FROM merchant_redirects WHERE merchant_id=ANY($1) ORDER BY retired_id",&[&ids])?.into_iter().map(|r|(r.get(0),r.get(1))).collect(),
        location_redirects:if include_locations {c.query("SELECT d.retired_id,d.location_id FROM location_redirects d JOIN location_records l ON l.id=d.location_id WHERE l.merchant_id=ANY(merchant_keys($1)) ORDER BY d.retired_id",&[&ids])?.into_iter().map(|r|(r.get(0),r.get(1))).collect()} else {vec![]},
        revision:Some(c.query_one("SELECT revision FROM catalog_revision",&[])?.get(0)),
    })
}

#[cfg(test)]
#[path = "../tests/unit/postgres_store/connection_tests.rs"]
mod connection_tests;

#[cfg(test)]
include!("../tests/unit/postgres_store/support.rs");
