use crate::markets::{MarketIndex, market_counts, paginate};
use crate::search_profile::{Stage, timed};
use crate::{
    Merchant,
    location::{LocationRecord, MerchantReference},
};
use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

pub fn normalize(value: &str) -> String {
    // Most bank descriptors and aliases are ASCII. Avoid Unicode decomposition,
    // intermediate strings, and token vectors on that common path.
    if value.is_ascii() {
        let mut result = String::with_capacity(value.len());
        let mut separator = false;
        for byte in value.bytes() {
            if byte.is_ascii_alphanumeric() {
                if separator {
                    result.push(' ');
                    separator = false;
                }
                result.push(byte.to_ascii_lowercase() as char);
            } else if !result.is_empty() {
                separator = true;
            }
        }
        return result;
    }
    value
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone)]
struct SqliteStore(Arc<Mutex<Connection>>);

#[path = "postgres_store.rs"]
mod postgres_store;

#[derive(Clone)]
pub struct MerchantStore(Backend);
#[derive(Clone)]
enum Backend {
    Sqlite(SqliteStore),
    Postgres(postgres_store::PostgresStore),
}

impl MerchantStore {
    pub(crate) fn write_log(
        &self,
        id: &str,
        batch: &str,
        status: &str,
        merchant: Option<&str>,
        data: &Value,
    ) -> Result<()> {
        let data = serde_json::to_string(data)?;
        match &self.0 {
            Backend::Postgres(s) => s.write_log(id, batch, status, merchant, &data),
            Backend::Sqlite(s) => {
                let connection =
                    s.0.lock()
                        .map_err(|_| anyhow::anyhow!("database lock poisoned"))?;
                connection.execute("INSERT INTO enrichment_log(id,batch_id,status,merchant_id,data,finished_at) VALUES(?1,?2,?3,?4,?5,CASE WHEN ?3='started' THEN NULL ELSE strftime('%Y-%m-%dT%H:%M:%fZ','now') END) ON CONFLICT(id) DO UPDATE SET status=excluded.status,merchant_id=excluded.merchant_id,data=excluded.data,finished_at=excluded.finished_at", params![id,batch,status,merchant,data])?;
                Ok(())
            }
        }
    }
    /// Inspect private enrichment history, newest first. No provider call is made.
    pub fn enrichment_logs(
        &self,
        status: Option<&str>,
        merchant: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Value>> {
        if status.is_some_and(|s| !["started", "matched", "unresolved", "error"].contains(&s)) {
            bail!("unknown enrichment log status");
        }
        if limit == 0 || limit > 1000 || offset > i64::MAX as usize {
            bail!("log limit must be 1..1000 and offset must fit an integer");
        }
        match &self.0 {
            Backend::Postgres(s) => s.logs(status, merchant, limit, offset),
            Backend::Sqlite(s) => {
                let connection =
                    s.0.lock()
                        .map_err(|_| anyhow::anyhow!("database lock poisoned"))?;
                let mut stmt = connection.prepare("SELECT id,batch_id,status,merchant_id,created_at,finished_at,data FROM enrichment_log WHERE (?1 IS NULL OR status=?1) AND (?2 IS NULL OR merchant_id=?2) ORDER BY created_at DESC,id LIMIT ?3 OFFSET ?4")?;
                let rows = stmt.query_map(
                    params![status, merchant, limit as i64, offset as i64],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, Option<String>>(3)?,
                            r.get::<_, String>(4)?,
                            r.get::<_, Option<String>>(5)?,
                            r.get::<_, String>(6)?,
                        ))
                    },
                )?;
                rows.map(|r| { let (id,batch,status,merchant,created,finished,data) = r?; Ok(serde_json::json!({"id":id,"batch_id":batch,"status":status,"merchant_id":merchant,"created_at":created,"finished_at":finished,"data":serde_json::from_str::<Value>(&data)?})) }).collect()
            }
        }
    }
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self(Backend::Sqlite(SqliteStore::open(path)?)))
    }
    pub fn memory() -> Result<Self> {
        Ok(Self(Backend::Sqlite(SqliteStore::memory()?)))
    }
    pub fn postgres(url: &str) -> Result<Self> {
        Ok(Self(Backend::Postgres(
            postgres_store::PostgresStore::connect(url, false)?,
        )))
    }
    /// Connect on first catalog operation, so public pages/health don't wake an
    /// idle database. Schema/connection errors are reported by that operation.
    pub fn postgres_lazy(url: &str) -> Result<Self> {
        Ok(Self(Backend::Postgres(
            postgres_store::PostgresStore::lazy(url)?,
        )))
    }
    pub fn initialize_postgres(url: &str) -> Result<Self> {
        Ok(Self(Backend::Postgres(
            postgres_store::PostgresStore::connect(url, true)?,
        )))
    }
    pub fn configured(path: &Path, database_url: Option<&str>) -> Result<Self> {
        match database_url {
            Some(url) => Self::postgres(url),
            None => Self::open(path),
        }
    }
    /// Copy authoritative SQLite rows into an empty PostgreSQL database atomically.
    /// The source is opened read-only and is never modified.
    pub fn migrate_sqlite(&self, path: &Path) -> Result<usize> {
        let Backend::Postgres(store) = &self.0 else {
            bail!("migration target must be PostgreSQL");
        };
        let mut connection =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let transaction = connection.transaction()?;
        fn read(tx: &Transaction<'_>, sql: &str) -> Result<Vec<(String, String)>> {
            let mut stmt = tx.prepare(sql)?;
            Ok(stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?)
        }
        let mut merchants = read(&transaction, "SELECT id,data FROM merchants ORDER BY id")?;
        let mut manual = read(
            &transaction,
            "SELECT id,data FROM manual_merchants ORDER BY id",
        )?;
        let mut sources = read(
            &transaction,
            "SELECT merchant_id,data FROM source_records ORDER BY source,external_id",
        )?;
        let has_locations: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='location_records')", [], |r| r.get(0))?;
        let locations = if has_locations {
            read(
                &transaction,
                "SELECT id,data FROM location_records ORDER BY source,external_id",
            )?
        } else {
            vec![]
        };
        let version: i64 = transaction.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 3 {
            for (_, data) in merchants.iter_mut().chain(manual.iter_mut()) {
                *data = crate::markets::migrate_country(data, false)?;
            }
            for (_, data) in &mut sources {
                *data = crate::markets::migrate_country(data, true)?;
            }
        }
        let mut contents = Vec::new();
        for (_, data) in merchants.iter().chain(manual.iter()) {
            contents.extend(data.bytes());
            contents.push(0);
        }
        for (id, data) in &sources {
            contents.extend(data.bytes());
            contents.push(0);
            contents.extend(id.bytes());
            contents.push(0);
        }
        for (_, data) in &locations {
            contents.extend(data.bytes());
            contents.push(0);
        }
        store.restore(
            locations,
            merchants,
            manual,
            sources,
            crate::eval::fingerprint(&contents),
        )
    }
    /// Import reviewed outlets atomically; reimports preserve catalog IDs.
    pub fn import_locations(&self, records: &[LocationRecord]) -> Result<()> {
        let mut keys = HashSet::new();
        for record in records {
            record.validate()?;
            if !keys.insert((&record.source, &record.external_id)) {
                bail!("duplicate outlet source/external ID");
            }
        }
        match &self.0 {
            Backend::Postgres(store) => store.import_locations(records),
            Backend::Sqlite(store) => {
                let mut connection = store
                    .0
                    .lock()
                    .map_err(|_| anyhow::anyhow!("database lock poisoned"))?;
                let tx = connection.transaction()?;
                for record in records {
                    let existing: Option<String> = tx
                        .query_row(
                            "SELECT data FROM location_records WHERE source=?1 AND external_id=?2",
                            params![record.source, record.external_id],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if !record.manual_override
                        && existing
                            .as_deref()
                            .map(serde_json::from_str::<LocationRecord>)
                            .transpose()?
                            .is_some_and(|r| r.manual_override)
                    {
                        continue;
                    }
                    let (merchant_id, merchant_source, merchant_external) =
                        location_reference(&record.merchant);
                    let exists: bool = if let Some(id) = merchant_id {
                        tx.query_row(
                            "SELECT EXISTS(SELECT 1 FROM merchants WHERE id=?1)",
                            [id],
                            |r| r.get(0),
                        )?
                    } else {
                        tx.query_row("SELECT EXISTS(SELECT 1 FROM source_records WHERE source=?1 AND external_id=?2)", params![merchant_source,merchant_external], |r| r.get(0))?
                    };
                    if !exists {
                        bail!(
                            "outlet merchant reference is missing; import or link its merchant first"
                        );
                    }
                    let id: String = tx
                        .query_row(
                            "SELECT id FROM location_records WHERE source=?1 AND external_id=?2",
                            params![record.source, record.external_id],
                            |r| r.get(0),
                        )
                        .optional()?
                        .unwrap_or_else(|| format!("loc_{}", uuid::Uuid::new_v4().simple()));
                    let mut record = record.clone();
                    record.location.id = Some(id.clone());
                    tx.execute("INSERT INTO location_records(id,source,external_id,merchant_id,merchant_source,merchant_external_id,data) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(source,external_id) DO UPDATE SET merchant_id=excluded.merchant_id,merchant_source=excluded.merchant_source,merchant_external_id=excluded.merchant_external_id,data=excluded.data", params![id,record.source,record.external_id,merchant_id,merchant_source,merchant_external,serde_json::to_string(&record)?])?;
                }
                tx.commit()?;
                Ok(())
            }
        }
    }
    /// Fetch outlets for a merchant, following source links at read time.
    pub fn locations(&self, merchant_id: &str) -> Result<Vec<LocationRecord>> {
        match &self.0 {
            Backend::Postgres(store) => store.locations(merchant_id),
            Backend::Sqlite(store) => {
                let connection = store
                    .0
                    .lock()
                    .map_err(|_| anyhow::anyhow!("database lock poisoned"))?;
                let mut statement = connection.prepare("SELECT l.data FROM location_records l LEFT JOIN source_records s ON s.source=l.merchant_source AND s.external_id=l.merchant_external_id WHERE COALESCE(l.merchant_id,s.merchant_id)=?1 ORDER BY l.id")?;
                let rows = statement.query_map([merchant_id], |r| r.get::<_, String>(0))?;
                rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
            }
        }
    }

    pub fn put(&self, merchant: &Merchant) -> Result<()> {
        match &self.0 {
            Backend::Sqlite(s) => s.put(merchant),
            Backend::Postgres(s) => s.put(merchant),
        }
    }
    pub fn import(&self, records: &[SourceRecord]) -> Result<()> {
        match &self.0 {
            Backend::Sqlite(s) => s.import(records),
            Backend::Postgres(s) => s.import(records),
        }
    }
    pub fn link(&self, source: &str, external_id: &str, target: &str) -> Result<()> {
        match &self.0 {
            Backend::Sqlite(s) => s.link(source, external_id, target),
            Backend::Postgres(s) => s.link(source, external_id, target),
        }
    }
    pub fn resolve_source(&self, source: &str, external_id: &str) -> Result<Option<String>> {
        match &self.0 {
            Backend::Sqlite(s) => s.resolve_source(source, external_id),
            Backend::Postgres(s) => s.resolve_source(source, external_id),
        }
    }
    pub fn fingerprint(&self) -> Result<String> {
        match &self.0 {
            Backend::Sqlite(s) => s.fingerprint(),
            Backend::Postgres(s) => s.fingerprint(),
        }
    }
    /// Catalog counts from one consistent database snapshot.
    pub fn stats(&self) -> Result<MerchantStats> {
        match &self.0 {
            Backend::Sqlite(s) => s.stats(),
            Backend::Postgres(s) => s.stats(),
        }
    }
    pub fn list(&self, market: Option<&str>, limit: usize, offset: usize) -> Result<MerchantPage> {
        match &self.0 {
            Backend::Sqlite(s) => s.list(market, limit, offset),
            Backend::Postgres(s) => s.list(market, limit, offset),
        }
    }
    pub fn search(
        &self,
        description: &str,
        country: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Candidate>> {
        match &self.0 {
            Backend::Sqlite(s) => s.search(description, country, limit),
            Backend::Postgres(s) => s.search(description, country, limit),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub merchant: Merchant,
    pub score: f64,
    pub exact: bool,
    /// Number of descriptor characters matched by an Open Enrichment rule.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regex_match_length: Option<usize>,
    pub trusted: bool,
    pub provenance: Vec<SourceRecord>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MerchantPage {
    pub merchants: Vec<Merchant>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct MerchantStats {
    pub total: usize,
    /// Includes manual corrections to imported merchants.
    pub manual: usize,
    pub without_source: usize,
    pub by_source: Vec<MerchantSourceStats>,
    pub without_market_evidence: usize,
    pub by_market: Vec<MerchantMarketStats>,
    pub by_source_region: Vec<MerchantRegionStats>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct MerchantSourceStats {
    pub source: String,
    pub merchants: usize,
    pub records: usize,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct MerchantMarketStats {
    pub market: String,
    pub merchants: usize,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct MerchantRegionStats {
    pub source: String,
    pub region: String,
    pub merchants: usize,
    pub records: usize,
}

// Portable aggregate queries shared by SQLite and PostgreSQL.
const STATS_TOTALS: &str = "SELECT (SELECT COUNT(*) FROM merchants), (SELECT COUNT(*) FROM manual_merchants), (SELECT COUNT(*) FROM merchants m WHERE NOT EXISTS (SELECT 1 FROM source_records s WHERE s.merchant_id=m.id))";
const STATS_SOURCES: &str = "SELECT source,COUNT(DISTINCT merchant_id),COUNT(*) FROM source_records GROUP BY source ORDER BY COUNT(DISTINCT merchant_id) DESC,source";

/// An external record retained independently from manual merchant corrections.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRecord {
    pub source: String,
    pub external_id: String,
    pub merchant: Merchant,
    pub attribution: String,
    pub license: String,
    pub url: String,
    pub version: Option<String>,
    pub raw: Value,
}

impl SqliteStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        Self::initialize(Connection::open(path)?)
    }
    pub fn memory() -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?)
    }
    fn initialize(mut connection: Connection) -> Result<Self> {
        connection.busy_timeout(Duration::from_secs(3))?;
        connection.execute_batch("PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS enrichment_log(id TEXT PRIMARY KEY, batch_id TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN ('started','matched','unresolved','error')), merchant_id TEXT, created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')), finished_at TEXT, data TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS enrichment_log_created ON enrichment_log(created_at DESC,id);
            CREATE INDEX IF NOT EXISTS enrichment_log_status ON enrichment_log(status,created_at DESC);
            CREATE INDEX IF NOT EXISTS enrichment_log_merchant ON enrichment_log(merchant_id,created_at DESC);
            CREATE TABLE IF NOT EXISTS merchants(id TEXT PRIMARY KEY, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS aliases(merchant_id TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE, normalized TEXT NOT NULL, PRIMARY KEY(merchant_id, normalized));
            CREATE INDEX IF NOT EXISTS aliases_normalized ON aliases(normalized);
            CREATE VIRTUAL TABLE IF NOT EXISTS merchant_tokens USING fts5(merchant_id UNINDEXED, text, tokenize='unicode61');
            CREATE VIRTUAL TABLE IF NOT EXISTS merchant_trigrams USING fts5(merchant_id UNINDEXED, text, tokenize='trigram');")?;
        let transaction = connection.transaction()?;
        transaction.execute_batch("CREATE TABLE IF NOT EXISTS manual_merchants(id TEXT PRIMARY KEY, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS source_records(source TEXT NOT NULL, external_id TEXT NOT NULL, merchant_id TEXT NOT NULL REFERENCES merchants(id), data TEXT NOT NULL, PRIMARY KEY(source,external_id));
            CREATE INDEX IF NOT EXISTS source_records_merchant ON source_records(merchant_id);
            CREATE TABLE IF NOT EXISTS location_records(id TEXT PRIMARY KEY, source TEXT NOT NULL, external_id TEXT NOT NULL, merchant_id TEXT REFERENCES merchants(id), merchant_source TEXT, merchant_external_id TEXT, data TEXT NOT NULL, UNIQUE(source,external_id), FOREIGN KEY(merchant_source,merchant_external_id) REFERENCES source_records(source,external_id));
            CREATE INDEX IF NOT EXISTS location_records_merchant ON location_records(merchant_id);
            CREATE INDEX IF NOT EXISTS location_records_source_merchant ON location_records(merchant_source,merchant_external_id);")?;
        let version: i64 = transaction.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == 0 {
            transaction.execute(
                "INSERT OR IGNORE INTO manual_merchants SELECT id,data FROM merchants",
                [],
            )?;
            transaction.execute_batch("PRAGMA user_version=1;")?;
        }
        if version < 3 {
            for table in ["merchants", "manual_merchants", "source_records"] {
                let rows: Vec<(i64, String)> = transaction
                    .prepare(&format!("SELECT rowid,data FROM {table}"))?
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?;
                for (rowid, data) in rows {
                    let data = crate::markets::migrate_country(&data, table == "source_records")?;
                    transaction.execute(
                        &format!("UPDATE {table} SET data=?1 WHERE rowid=?2"),
                        params![data, rowid],
                    )?;
                }
            }
            let has_country: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('merchants') WHERE name='country')",
                [],
                |r| r.get(0),
            )?;
            if has_country {
                transaction.execute_batch("ALTER TABLE merchants DROP COLUMN country;")?;
            }
            transaction.execute_batch("PRAGMA user_version=3;")?;
        }
        transaction.commit()?;
        Ok(Self(Arc::new(Mutex::new(connection))))
    }
    pub fn put(&self, merchant: &Merchant) -> Result<()> {
        validate(merchant)?;
        let mut connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("merchant database lock failed"))?;
        let transaction = connection.transaction()?;
        transaction.execute("INSERT INTO manual_merchants VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data", params![merchant.id,serde_json::to_string(merchant)?])?;
        rebuild(&transaction, &merchant.id)?;
        transaction.commit()?;
        Ok(())
    }

    /// The source/external ID pair retains the same local merchant ID across refreshes.
    /// Validate the entire batch before changing anything; refreshes are atomic.
    pub fn import(&self, records: &[SourceRecord]) -> Result<()> {
        let mut keys = HashSet::new();
        for record in records {
            validate(&record.merchant)?;
            if record.source.trim().is_empty()
                || record.external_id.trim().is_empty()
                || !keys.insert((&record.source, &record.external_id))
            {
                bail!("source records must have unique nonblank source/external ID pairs");
            }
        }
        let mut connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("merchant database lock failed"))?;
        let transaction = connection.transaction()?;
        for record in records {
            let existing: Option<String> = transaction
                .query_row(
                    "SELECT merchant_id FROM source_records WHERE source=?1 AND external_id=?2",
                    params![record.source, record.external_id],
                    |r| r.get(0),
                )
                .optional()?;
            let id = existing.unwrap_or_else(|| format!("mer_{}", uuid::Uuid::new_v4().simple()));
            transaction.execute(
                "INSERT OR IGNORE INTO merchants(id,data) VALUES(?1,?2)",
                params![id, serde_json::to_string(&record.merchant)?],
            )?;
            transaction.execute("INSERT INTO source_records VALUES(?1,?2,?3,?4) ON CONFLICT(source,external_id) DO UPDATE SET data=excluded.data", params![record.source,record.external_id,id,serde_json::to_string(record)?])?;
            rebuild(&transaction, &id)?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Explicitly associate an external record with a known local merchant.
    pub fn link(&self, source: &str, external_id: &str, target: &str) -> Result<()> {
        let mut connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("merchant database lock failed"))?;
        let transaction = connection.transaction()?;
        let old: String = transaction.query_row(
            "SELECT merchant_id FROM source_records WHERE source=?1 AND external_id=?2",
            params![source, external_id],
            |r| r.get(0),
        )?;
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM merchants WHERE id=?1)",
            [target],
            |r| r.get(0),
        )?;
        if !exists {
            bail!("target merchant does not exist");
        }
        transaction.execute(
            "UPDATE source_records SET merchant_id=?3 WHERE source=?1 AND external_id=?2",
            params![source, external_id, target],
        )?;
        rebuild(&transaction, target)?;
        if old != target {
            rebuild(&transaction, &old)?;
        }
        transaction.commit()?;
        Ok(())
    }
    pub fn resolve_source(&self, source: &str, external_id: &str) -> Result<Option<String>> {
        let connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("merchant database lock failed"))?;
        Ok(connection
            .query_row(
                "SELECT merchant_id FROM source_records WHERE source=?1 AND external_id=?2",
                params![source, external_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn fingerprint(&self) -> Result<String> {
        let connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("merchant database lock failed"))?;
        let mut contents = Vec::new();
        for sql in [
            "SELECT data FROM merchants ORDER BY id",
            "SELECT data FROM manual_merchants ORDER BY id",
            "SELECT data,merchant_id FROM source_records ORDER BY source,external_id",
            "SELECT data FROM location_records ORDER BY source,external_id",
        ] {
            let mut statement = connection.prepare_cached(sql)?;
            for row in statement.query_map([], |r| {
                (0..r.as_ref().column_count())
                    .map(|column| r.get::<_, String>(column))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })? {
                for value in row? {
                    contents.extend(value.bytes());
                    contents.push(0);
                }
            }
        }
        Ok(crate::eval::fingerprint(&contents))
    }

    pub fn stats(&self) -> Result<MerchantStats> {
        let mut connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("merchant database lock failed"))?;
        let tx = connection.transaction()?;
        let (total, manual, without_source): (i64, i64, i64) =
            tx.query_row(STATS_TOTALS, [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let mut by_source = Vec::new();
        for row in tx.prepare_cached(STATS_SOURCES)?.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })? {
            let (source, merchants, records) = row?;
            by_source.push(MerchantSourceStats {
                source,
                merchants: usize::try_from(merchants)?,
                records: usize::try_from(records)?,
            });
        }
        let index = sqlite_markets(&tx, None)?;
        let merchants = sqlite_catalog(&tx, &index)?;
        let (without_market_evidence, by_market) = market_counts(&merchants);
        let by_source_region = index.region_stats();
        tx.commit()?;
        Ok(MerchantStats {
            total: usize::try_from(total)?,
            manual: usize::try_from(manual)?,
            without_source: usize::try_from(without_source)?,
            by_source,
            without_market_evidence,
            by_market,
            by_source_region,
        })
    }

    /// Browse stored merchants by name, with deterministic pagination.
    pub fn list(&self, market: Option<&str>, limit: usize, offset: usize) -> Result<MerchantPage> {
        if !(1..=1000).contains(&limit) {
            bail!("limit must be between 1 and 1000");
        }
        if market
            .is_some_and(|value| value.len() != 2 || !value.bytes().all(|c| c.is_ascii_uppercase()))
        {
            bail!("market must be a two-letter uppercase code");
        }
        let sql_offset = i64::try_from(offset)?;
        let mut connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("merchant database lock failed"))?;
        let tx = connection.transaction()?;
        let page = if market.is_some() {
            let index = sqlite_markets(&tx, None)?;
            paginate(sqlite_catalog(&tx, &index)?, market, limit, offset)
        } else {
            let total: i64 = tx.query_row("SELECT COUNT(*) FROM merchants", [], |r| r.get(0))?;
            let mut statement = tx.prepare_cached("SELECT data FROM merchants ORDER BY json_extract(data,'$.name') COLLATE NOCASE,id LIMIT ?1 OFFSET ?2")?;
            let mut merchants = Vec::new();
            for data in
                statement.query_map(params![limit as i64, sql_offset], |r| r.get::<_, String>(0))?
            {
                let merchant: Merchant = serde_json::from_str(&data?)?;
                merchants.push(merchant);
            }
            let ids: Vec<_> = merchants.iter().map(|m| m.id.clone()).collect();
            let index = sqlite_markets(&tx, Some(&ids))?;
            let merchants = merchants.into_iter().map(|m| index.hydrate(m)).collect();
            MerchantPage {
                merchants,
                total: usize::try_from(total)?,
                limit,
                offset,
            }
        };
        tx.commit()?;
        Ok(page)
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
        let mut guard = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("merchant database lock failed"))?;
        let connection = guard.transaction()?;
        let mut found: HashMap<String, Candidate> = HashMap::new();
        let exact_rows = timed(Stage::ExactSql, || -> Result<Vec<String>> {
            let mut exact = connection.prepare_cached("SELECT m.data FROM merchants m JOIN aliases a ON a.merchant_id=m.id WHERE a.normalized=?1 LIMIT 255")?;
            Ok(exact
                .query_map(params![query], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<_>>()?)
        })?;
        for data in exact_rows {
            let merchant: Merchant = timed(Stage::Decode, || serde_json::from_str(&data))?;
            let (trusted, provenance) = timed(Stage::Evidence, || {
                evidence(&connection, &merchant.id, &query)
            })?;
            if excluded(&query, &provenance) && !trusted {
                continue;
            }
            found.insert(
                merchant.id.clone(),
                Candidate {
                    merchant,
                    score: 1.0,
                    exact: true,
                    regex_match_length: None,
                    trusted,
                    provenance,
                },
            );
        }
        // Regexes must generate candidates independently of alias/token recall.
        // Read authoritative source rows on every search so imports and links
        // from other processes become visible immediately, including old bundles.
        let mut rules = connection.prepare_cached("SELECT s.merchant_id,json_extract(s.data,'$.raw.transaction_text_regexp') FROM source_records s JOIN merchants m ON m.id=s.merchant_id WHERE s.source='open-enrichment' AND coalesce(json_extract(s.data,'$.raw.parent_id'),'')='' AND json_type(s.data,'$.raw.transaction_text_regexp')='text'")?;
        let rows = rules.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut matches: HashMap<String, usize> = HashMap::new();
        for row in rows {
            let (id, pattern) = row?;
            if let Some(length) = crate::regex_rules::match_length(&pattern, description) {
                matches
                    .entry(id)
                    .and_modify(|v| *v = (*v).max(length))
                    .or_insert(length);
            }
        }
        for (id, length) in matches {
            if let Some(candidate) = found.get_mut(&id) {
                candidate.regex_match_length = Some(length);
                continue;
            }
            let data: String =
                connection.query_row("SELECT data FROM merchants WHERE id=?1", [&id], |r| {
                    r.get(0)
                })?;
            let merchant = serde_json::from_str(&data)?;
            let (trusted, provenance) = evidence(&connection, &id, &query)?;
            if !excluded(&query, &provenance) || trusted {
                found.insert(
                    id,
                    Candidate {
                        merchant,
                        score: 1.0,
                        exact: false,
                        regex_match_length: Some(length),
                        trusted,
                        provenance,
                    },
                );
            }
        }
        let tokens: Vec<_> = query
            .split_whitespace()
            .filter(|t| t.chars().count() >= 3)
            .take(16)
            .collect();
        let token_query = tokens
            .iter()
            .map(|token| format!("\"{token}\"*"))
            .collect::<Vec<_>>()
            .join(" OR ");
        let mut grams = HashSet::new();
        for token in tokens {
            let chars: Vec<_> = token.chars().collect();
            for window in chars.windows(3) {
                if grams.len() < 64 {
                    grams.insert(window.iter().collect::<String>());
                }
            }
        }
        let gram_query = grams
            .iter()
            .map(|g| format!("\"{g}\""))
            .collect::<Vec<_>>()
            .join(" OR ");
        let mut seen: HashSet<String> = found.keys().cloned().collect();
        let mut scorer = crate::search_score::Scorer::new(&query);
        for (table, expression) in [
            ("merchant_tokens", token_query),
            ("merchant_trigrams", gram_query),
        ] {
            if expression.is_empty() {
                continue;
            }
            let sql = format!(
                "SELECT m.id,m.data FROM {table} JOIN merchants m ON m.id={table}.merchant_id WHERE {table} MATCH ?1 ORDER BY {table}.rank LIMIT 100"
            );
            let stage = if table == "merchant_tokens" {
                Stage::TokenSql
            } else {
                Stage::TrigramSql
            };
            let rows = timed(stage, || -> Result<Vec<(String, String)>> {
                let mut statement = connection.prepare_cached(&sql)?;
                Ok(statement
                    .query_map(params![expression], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?)
            })?;
            for (id, data) in rows {
                if !seen.insert(id) {
                    continue;
                }
                let merchant: Merchant = timed(Stage::Decode, || serde_json::from_str(&data))?;
                let score = timed(Stage::Score, || {
                    std::iter::once(&merchant.name)
                        .chain(merchant.aliases.iter())
                        .map(|name| scorer.score(normalize(name)))
                        .fold(0.0, f64::max)
                });
                if score < 0.35 {
                    continue;
                }
                let (trusted, provenance) = timed(Stage::Evidence, || {
                    evidence(&connection, &merchant.id, &query)
                })?;
                if excluded(&query, &provenance) && !trusted {
                    continue;
                }
                if score >= 0.35 {
                    found.entry(merchant.id.clone()).or_insert(Candidate {
                        merchant,
                        score,
                        exact: false,
                        regex_match_length: None,
                        trusted,
                        provenance,
                    });
                }
            }
        }
        let ids: Vec<_> = found.keys().cloned().collect();
        let index = sqlite_markets(&connection, Some(&ids))?;
        for candidate in found.values_mut() {
            candidate.merchant = index.hydrate(candidate.merchant.clone());
        }
        Ok(rank_candidates(found, limit, country))
    }
}
fn sqlite_markets(connection: &Connection, ids: Option<&[String]>) -> Result<MarketIndex> {
    let mut index = MarketIndex::default();
    let ids = ids.map(serde_json::to_string).transpose()?;
    let filter = |column: &str| {
        if ids.is_some() {
            format!("{column} IN (SELECT value FROM json_each(?1))")
        } else {
            "?1 IS NULL".into()
        }
    };
    let manual_sql = format!(
        "SELECT id,data FROM manual_merchants WHERE {}",
        filter("id")
    );
    for row in connection
        .prepare_cached(&manual_sql)?
        .query_map([&ids], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
    {
        let (id, data) = row?;
        index.declaration(&id, &serde_json::from_str(&data)?, "manual", None);
    }
    let sources_sql = format!(
        "SELECT merchant_id,data FROM source_records WHERE {} ORDER BY source,external_id",
        filter("merchant_id")
    );
    for row in connection
        .prepare_cached(&sources_sql)?
        .query_map([&ids], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
    {
        let (id, data) = row?;
        index.source(&id, &serde_json::from_str(&data)?);
    }
    let outlets_sql = if ids.is_some() {
        format!(
            "SELECT merchant_id,data FROM location_records WHERE {} UNION ALL SELECT s.merchant_id,l.data FROM source_records s JOIN location_records l ON s.source=l.merchant_source AND s.external_id=l.merchant_external_id WHERE l.merchant_id IS NULL AND {}",
            filter("merchant_id"),
            filter("s.merchant_id")
        )
    } else {
        "SELECT COALESCE(l.merchant_id,s.merchant_id),l.data FROM location_records l LEFT JOIN source_records s ON s.source=l.merchant_source AND s.external_id=l.merchant_external_id WHERE ?1 IS NULL".into()
    };
    for row in connection
        .prepare_cached(&outlets_sql)?
        .query_map([&ids], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
    {
        let (id, data) = row?;
        index.outlet(&id, &serde_json::from_str(&data)?);
    }
    Ok(index)
}
fn sqlite_catalog(connection: &Connection, index: &MarketIndex) -> Result<Vec<Merchant>> {
    connection
        .prepare_cached("SELECT data FROM merchants")?
        .query_map([], |r| r.get::<_, String>(0))?
        .map(|data| Ok(index.hydrate(serde_json::from_str(&data?)?)))
        .collect()
}

fn rank_candidates(
    found: HashMap<String, Candidate>,
    limit: usize,
    country: Option<&str>,
) -> Vec<Candidate> {
    let mut results: Vec<_> = found.into_values().collect();
    results.sort_by(|a, b| {
        b.exact
            .cmp(&a.exact)
            .then_with(|| b.regex_match_length.cmp(&a.regex_match_length))
            .then_with(|| b.score.total_cmp(&a.score))
            .then_with(|| {
                let known = |c: &Candidate| {
                    country.is_some_and(|country| c.merchant.markets.iter().any(|m| m == country))
                };
                known(b).cmp(&known(a))
            })
            .then_with(|| a.merchant.id.cmp(&b.merchant.id))
    });
    // Preserve exact collisions and equally specific regex hits even at limit=1.
    let exact_count = results.iter().take_while(|r| r.exact).count();
    let regex_ties = results
        .first()
        .filter(|r| !r.exact)
        .and_then(|r| r.regex_match_length)
        .map_or(0, |length| {
            results
                .iter()
                .take_while(|r| r.regex_match_length == Some(length))
                .count()
        });
    results.truncate(exact_count.max(regex_ties).max(limit).min(254));
    results
}
pub(crate) fn validate(merchant: &Merchant) -> Result<()> {
    if merchant.id.trim().is_empty() || normalize(&merchant.name).is_empty() {
        bail!("merchant ID and name must be nonblank");
    }
    if merchant
        .markets
        .iter()
        .any(|c| !crate::markets::valid_country(c))
    {
        bail!("markets must contain two-letter uppercase country codes");
    }
    for evidence in &merchant.market_evidence {
        if !crate::markets::valid_country(&evidence.country) || evidence.source.trim().is_empty() {
            bail!("market evidence must have a valid country and nonblank source");
        }
    }
    if let Some(logo) = &merchant.logo_url {
        let url = reqwest::Url::parse(logo)
            .map_err(|_| anyhow::anyhow!("logo_url must be an absolute HTTP(S) URL"))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            bail!("logo_url must be an absolute HTTP(S) URL without credentials");
        }
    }
    if merchant.aliases.iter().any(|a| normalize(a).is_empty()) {
        bail!("aliases must be nonblank");
    }
    Ok(())
}

fn source_records(connection: &Connection, id: &str) -> Result<Vec<SourceRecord>> {
    let mut statement = connection.prepare_cached(
        "SELECT data FROM source_records WHERE merchant_id=?1 ORDER BY source,external_id",
    )?;
    let mut records = Vec::new();
    for data in statement.query_map([id], |r| r.get::<_, String>(0))? {
        records.push(serde_json::from_str(&data?)?);
    }
    Ok(records)
}
fn evidence(connection: &Connection, id: &str, query: &str) -> Result<(bool, Vec<SourceRecord>)> {
    let data: Option<String> = connection
        .prepare_cached("SELECT data FROM manual_merchants WHERE id=?1")?
        .query_row([id], |r| r.get(0))
        .optional()?;
    let trusted = if let Some(data) = data {
        let manual: Merchant = serde_json::from_str(&data)?;
        std::iter::once(&manual.name)
            .chain(manual.aliases.iter())
            .any(|name| normalize(name) == query)
    } else {
        false
    };
    Ok((trusted, source_records(connection, id)?))
}
fn excluded(query: &str, records: &[SourceRecord]) -> bool {
    // Imported negative aliases are whole phrases, never arbitrary substrings.
    let query = format!(" {query} ");
    records.iter().any(|r| {
        r.raw["negativeAliases"].as_array().is_some_and(|aliases| {
            aliases.iter().filter_map(Value::as_str).any(|a| {
                let a = normalize(a);
                !a.is_empty() && query.contains(&format!(" {a} "))
            })
        })
    })
}
fn rebuild(transaction: &Transaction<'_>, id: &str) -> Result<()> {
    let manual: Option<String> = transaction
        .query_row("SELECT data FROM manual_merchants WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .optional()?;
    let records = source_records(transaction, id)?;
    let mut merchant: Option<Merchant> =
        manual.map(|data| serde_json::from_str(&data)).transpose()?;
    for record in &records {
        if merchant.is_none() {
            merchant = Some(record.merchant.clone());
        }
        let m = merchant.as_mut().unwrap();
        m.aliases.extend(
            std::iter::once(record.merchant.name.clone()).chain(record.merchant.aliases.clone()),
        );
        if !record.url.is_empty() {
            m.sources.push(record.url.clone());
        }
    }
    for table in ["aliases", "merchant_tokens", "merchant_trigrams"] {
        transaction.execute(&format!("DELETE FROM {table} WHERE merchant_id=?1"), [id])?;
    }
    let Some(mut merchant) = merchant else {
        transaction.execute("DELETE FROM merchants WHERE id=?1", [id])?;
        return Ok(());
    };
    merchant.id = id.into();
    merchant.aliases.sort();
    merchant.aliases.dedup();
    merchant.sources.sort();
    merchant.sources.dedup();
    transaction.execute("INSERT INTO merchants(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data", params![id,serde_json::to_string(&merchant)?])?;
    let names: HashSet<_> = std::iter::once(&merchant.name)
        .chain(merchant.aliases.iter())
        .map(|n| normalize(n))
        .collect();
    for name in &names {
        transaction.execute("INSERT INTO aliases VALUES(?1,?2)", params![id, name])?;
    }
    let text = names.into_iter().collect::<Vec<_>>().join(" \n ");
    for table in ["merchant_tokens", "merchant_trigrams"] {
        transaction.execute(
            &format!("INSERT INTO {table}(merchant_id,text) VALUES(?1,?2)"),
            params![id, text],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn regex_search_scenarios(store: &MerchantStore) -> Result<()> {
        let rule = |id: &str, pattern: &str| {
            let mut r = record(id, &format!("Rule merchant {id}"));
            r.source = "open-enrichment".into();
            r.raw = serde_json::json!({"transaction_text_regexp":pattern, "parent_id":""});
            r
        };
        let broad = rule("regex-broad", r"(?i)^ZXQ(?: |\s)M");
        let specific = rule("regex-specific", r"(?i)^ZXQ ME UP");
        store.import(&[
            broad.clone(),
            specific.clone(),
            rule("regex-invalid", "["),
            rule("regex-empty", ".*"),
        ])?;
        let specific_id = store
            .resolve_source("open-enrichment", "regex-specific")?
            .unwrap();
        let broad_id = store
            .resolve_source("open-enrichment", "regex-broad")?
            .unwrap();
        let hits = store.search("sq * ZXQ ME UP 9876", Some("CA"), 10)?;
        assert_eq!(hits[0].merchant.id, specific_id);
        assert_eq!(hits[0].regex_match_length, Some(9));
        assert_eq!(hits[1].merchant.id, broad_id);
        assert_eq!(hits[1].regex_match_length, Some(5));
        assert!(!hits[0].exact && !hits[0].trusted);
        assert_eq!(hits[0].provenance[0].external_id, "regex-specific");
        assert!(
            store
                .search("ZXQ ME UP 9876", Some("US"), 10)?
                .iter()
                .any(|c| c.regex_match_length.is_some())
        );
        let tied = rule("regex-tied", r"(?i)^ZXQ ME UP");
        store.import(&[tied])?;
        assert_eq!(store.search("ZXQ ME UP 9876", None, 1)?.len(), 2);
        let mut refreshed = specific.clone();
        refreshed.raw["transaction_text_regexp"] = serde_json::json!(r"(?i)^REPLACEMENT\b");
        store.import(&[refreshed])?;
        assert!(
            !store
                .search("ZXQ ME UP 9876", None, 10)?
                .iter()
                .any(|c| c.merchant.id == specific_id && c.regex_match_length.is_some())
        );
        assert_eq!(
            store.search("REPLACEMENT 9876", None, 10)?[0].merchant.id,
            specific_id
        );
        store.link("open-enrichment", "regex-broad", &specific_id)?;
        assert_eq!(
            store.search("ZXQ MARATHON 9876", None, 10)?[0].merchant.id,
            specific_id
        );
        let mut negative = rule("regex-negative", r"^EXCLUDED\b");
        negative.raw["negativeAliases"] = serde_json::json!(["EXCLUDED TEST"]);
        store.import(&[negative])?;
        assert!(
            store
                .search("EXCLUDED TEST", None, 10)?
                .iter()
                .all(|c| c.regex_match_length.is_none())
        );
        let mut child = rule("regex-child", "CHILDRULE");
        child.raw["parent_id"] = serde_json::json!("some-parent");
        store.import(&[child])?;
        assert!(
            store
                .search("CHILDRULE", None, 10)?
                .iter()
                .all(|c| c.regex_match_length.is_none())
        );
        let mut manual = merchant("regex-manual", "Manually verified", "CA");
        manual.aliases = vec!["ZXQ ME UP 9876".into()];
        store.put(&manual)?;
        let hits = store.search("ZXQ ME UP 9876", None, 10)?;
        assert_eq!(hits[0].merchant.id, manual.id);
        assert!(hits[0].trusted && hits[0].exact);
        Ok(())
    }
    #[test]
    fn open_enrichment_regex_search() -> Result<()> {
        regex_search_scenarios(&MerchantStore::memory()?)
    }
    fn merchant(id: &str, name: &str, country: &str) -> Merchant {
        Merchant {
            id: id.into(),
            name: name.into(),
            markets: vec![country.into()],
            market_evidence: vec![],
            website: None,
            logo_url: None,
            logo_source: None,
            aliases: vec![],
            sources: vec![],
        }
    }
    fn record(id: &str, name: &str) -> SourceRecord {
        SourceRecord {
            source: "test".into(),
            external_id: id.into(),
            merchant: merchant(id, name, "CA"),
            attribution: "Test data".into(),
            license: "test".into(),
            url: "https://example.com".into(),
            version: Some("1".into()),
            raw: serde_json::json!({}),
        }
    }
    #[test]
    fn ascii_normalization_matches_unicode_reference() {
        fn reference(value: &str) -> String {
            value
                .nfkd()
                .filter(|c| !is_combining_mark(*c))
                .flat_map(char::to_lowercase)
                .map(|c| if c.is_alphanumeric() { c } else { ' ' })
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        }
        let ascii: String = (0..=127).map(char::from).collect();
        for value in [
            &ascii,
            "  A--B___123 \tZ!",
            "",
            "---",
            "Julius Café",
            "İß 東京",
        ] {
            assert_eq!(normalize(value), reference(value));
        }
    }
    #[test]
    fn imports_are_untrusted_idempotent_and_preserve_manual_overrides() {
        let db = MerchantStore::memory().unwrap();
        let mut imported = record("external-1", "Julius Cafe");
        imported.merchant.aliases = vec!["BANK ORIGINAL".into()];
        db.import(&[imported.clone()]).unwrap();
        let first = db.search("BANK ORIGINAL", Some("CA"), 10).unwrap();
        assert!(first[0].exact);
        assert!(!first[0].trusted);
        let id = first[0].merchant.id.clone();
        db.import(&[imported.clone()]).unwrap();
        let second = db.search("BANK ORIGINAL", Some("CA"), 10).unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].merchant.id, id);
        let mut manual = merchant(&id, "Julius Café corrected", "CA");
        manual.aliases = vec!["VERIFIED ALIAS".into()];
        db.put(&manual).unwrap();
        imported.merchant.name = "Bad refresh name".into();
        imported.merchant.aliases = vec!["NEW IMPORTED ALIAS".into()];
        db.import(&[imported]).unwrap();
        let refreshed = db.search("NEW IMPORTED ALIAS", Some("CA"), 10).unwrap();
        assert_eq!(refreshed[0].merchant.name, "Julius Café corrected");
        assert!(!refreshed[0].trusted);
        assert!(db.search("VERIFIED ALIAS", Some("CA"), 10).unwrap()[0].trusted);
        assert!(
            !db.search("BANK ORIGINAL", Some("CA"), 10)
                .unwrap()
                .iter()
                .any(|c| c.exact)
        );
    }
    #[test]
    fn source_links_survive_refresh_and_import_failure_is_atomic() {
        let db = MerchantStore::memory().unwrap();
        let a = record("external-a", "Alpha merchant");
        let b = record("external-b", "Beta merchant");
        db.import(&[a.clone(), b.clone()]).unwrap();
        let target = db.search("Alpha merchant", None, 10).unwrap()[0]
            .merchant
            .id
            .clone();
        db.link("test", "external-b", &target).unwrap();
        db.import(&[b]).unwrap();
        let linked = db.search("Beta merchant", None, 10).unwrap();
        assert_eq!(linked[0].merchant.id, target);
        assert_eq!(linked[0].provenance.len(), 2);
        let mut bad = record("bad", " ");
        bad.merchant.id = "".into();
        assert!(db.import(&[record("new", "Brand new"), bad]).is_err());
        assert!(
            !db.search("Brand new", None, 10)
                .unwrap()
                .iter()
                .any(|c| c.exact)
        );
    }
    #[test]
    fn negative_aliases_exclude_imported_candidates() {
        let db = MerchantStore::memory().unwrap();
        let mut a = record("amazon", "Amazon");
        a.merchant.aliases = vec!["Amazon web services".into()];
        a.raw = serde_json::json!({"negativeAliases":["amazon web services"]});
        db.import(&[a]).unwrap();
        assert!(
            db.search("Amazon web services", None, 10)
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn normalization_fuzzy_retrieval_and_market_preference() {
        let db = MerchantStore::memory().unwrap();
        db.put(&merchant("a", "Julius Café", "CA")).unwrap();
        db.put(&merchant("b", "Julius Café", "US")).unwrap();
        let exact = db.search("JULIUS CAFE", Some("CA"), 10).unwrap();
        assert_eq!(exact[0].merchant.id, "a");
        assert!(exact[0].exact);
        assert_eq!(exact.len(), 2);
        assert_eq!(
            db.search("Julus cafe", Some("CA"), 10).unwrap()[0]
                .merchant
                .id,
            "a"
        );
        assert_eq!(
            db.search("Juli", Some("CA"), 10).unwrap()[0].merchant.id,
            "a"
        );
        assert!(db.search("LS", Some("CA"), 10).unwrap().is_empty());
        assert_eq!(db.search("Julius cafe", None, 1).unwrap().len(), 2);
    }
    #[test]
    fn updates_replace_aliases_and_handle_query_syntax_as_data() {
        let db = MerchantStore::memory().unwrap();
        let mut m = merchant("a", "Julius Café", "CA");
        m.aliases = vec!["OLD DESCRIPTION".into()];
        db.put(&m).unwrap();
        m.aliases = vec!["NEW DESCRIPTION".into()];
        db.put(&m).unwrap();
        assert!(
            !db.search("OLD DESCRIPTION", None, 10)
                .unwrap()
                .iter()
                .any(|c| c.exact)
        );
        assert!(db.search("NEW DESCRIPTION", None, 10).unwrap()[0].exact);
        db.search("\" OR * (NEAR) --", None, 10).unwrap();
    }
}

#[cfg(test)]
#[path = "postgres_tests.rs"]
mod postgres_tests;

/// Aggregate SQLite search timings when ULTRAFINANCE_PROFILE_SEARCH=1.
pub fn search_profile() -> Option<Value> {
    crate::search_profile::report()
}

pub(crate) fn location_reference(
    reference: &MerchantReference,
) -> (Option<&str>, Option<&str>, Option<&str>) {
    match reference {
        MerchantReference::Local { merchant_id } => (Some(merchant_id), None, None),
        MerchantReference::Source {
            source,
            external_id,
        } => (None, Some(source), Some(external_id)),
    }
}
