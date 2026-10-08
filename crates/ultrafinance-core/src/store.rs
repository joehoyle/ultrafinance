use crate::Merchant;
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
pub struct MerchantStore(Arc<Mutex<Connection>>);

#[derive(Debug, Serialize)]
pub struct Candidate {
    pub merchant: Merchant,
    pub score: f64,
    pub exact: bool,
    pub trusted: bool,
    pub provenance: Vec<SourceRecord>,
}

#[derive(Debug, Serialize)]
pub struct MerchantPage {
    pub merchants: Vec<Merchant>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
}

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

impl MerchantStore {
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
            CREATE TABLE IF NOT EXISTS merchants(id TEXT PRIMARY KEY, country TEXT, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS aliases(merchant_id TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE, normalized TEXT NOT NULL, PRIMARY KEY(merchant_id, normalized));
            CREATE INDEX IF NOT EXISTS aliases_normalized ON aliases(normalized);
            CREATE VIRTUAL TABLE IF NOT EXISTS merchant_tokens USING fts5(merchant_id UNINDEXED, text, tokenize='unicode61');
            CREATE VIRTUAL TABLE IF NOT EXISTS merchant_trigrams USING fts5(merchant_id UNINDEXED, text, tokenize='trigram');")?;
        let transaction = connection.transaction()?;
        transaction.execute_batch("CREATE TABLE IF NOT EXISTS manual_merchants(id TEXT PRIMARY KEY, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS source_records(source TEXT NOT NULL, external_id TEXT NOT NULL, merchant_id TEXT NOT NULL REFERENCES merchants(id), data TEXT NOT NULL, PRIMARY KEY(source,external_id));
            CREATE INDEX IF NOT EXISTS source_records_merchant ON source_records(merchant_id);")?;
        let version: i64 = transaction.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == 0 {
            transaction.execute(
                "INSERT OR IGNORE INTO manual_merchants SELECT id,data FROM merchants",
                [],
            )?;
            transaction.execute_batch("PRAGMA user_version=1;")?;
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
                "INSERT OR IGNORE INTO merchants VALUES(?1,NULL,?2)",
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
            "SELECT data FROM source_records ORDER BY source,external_id",
        ] {
            let mut statement = connection.prepare(sql)?;
            for row in statement.query_map([], |r| r.get::<_, String>(0))? {
                contents.extend(row?.bytes());
                contents.push(0);
            }
        }
        Ok(crate::eval::fingerprint(&contents))
    }

    /// Browse stored merchants by name, with deterministic pagination.
    pub fn list(&self, country: Option<&str>, limit: usize, offset: usize) -> Result<MerchantPage> {
        if !(1..=1000).contains(&limit) {
            bail!("limit must be between 1 and 1000");
        }
        if country
            .is_some_and(|value| value.len() != 2 || !value.bytes().all(|c| c.is_ascii_uppercase()))
        {
            bail!("country must be a two-letter uppercase code");
        }
        let sql_offset = i64::try_from(offset)?;
        let connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("merchant database lock failed"))?;
        let total: i64 = connection.query_row(
            "SELECT COUNT(*) FROM merchants WHERE (?1 IS NULL OR country=?1)",
            [country],
            |row| row.get(0),
        )?;
        let mut statement=connection.prepare("SELECT data FROM merchants WHERE (?1 IS NULL OR country=?1) ORDER BY json_extract(data,'$.name') COLLATE NOCASE,id LIMIT ?2 OFFSET ?3")?;
        let mut merchants = Vec::new();
        for data in statement.query_map(params![country, limit as i64, sql_offset], |row| {
            row.get::<_, String>(0)
        })? {
            merchants.push(serde_json::from_str(&data?)?);
        }
        Ok(MerchantPage {
            merchants,
            total: usize::try_from(total)?,
            limit,
            offset,
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
        let connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("merchant database lock failed"))?;
        let mut found: HashMap<String, Candidate> = HashMap::new();
        let mut exact = connection.prepare("SELECT m.data FROM merchants m JOIN aliases a ON a.merchant_id=m.id WHERE a.normalized=?1 AND (?2 IS NULL OR m.country IS NULL OR m.country=?2) LIMIT 255")?;
        for data in exact.query_map(params![query, country], |row| row.get::<_, String>(0))? {
            let merchant: Merchant = serde_json::from_str(&data?)?;
            let (trusted, provenance) = evidence(&connection, &merchant.id, &query)?;
            if excluded(&query, &provenance) && !trusted {
                continue;
            }
            found.insert(
                merchant.id.clone(),
                Candidate {
                    merchant,
                    score: 1.0,
                    exact: true,
                    trusted,
                    provenance,
                },
            );
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
        for (table, expression) in [
            ("merchant_tokens", token_query),
            ("merchant_trigrams", gram_query),
        ] {
            if expression.is_empty() {
                continue;
            }
            let sql = format!(
                "SELECT m.data FROM {table} JOIN merchants m ON m.id={table}.merchant_id WHERE {table} MATCH ?1 AND (?2 IS NULL OR m.country IS NULL OR m.country=?2) ORDER BY {table}.rank LIMIT 100"
            );
            let mut statement = connection.prepare(&sql)?;
            for data in
                statement.query_map(params![expression, country], |row| row.get::<_, String>(0))?
            {
                let merchant: Merchant = serde_json::from_str(&data?)?;
                let score = std::iter::once(&merchant.name)
                    .chain(merchant.aliases.iter())
                    .map(|name| similarity(&query, &normalize(name)))
                    .fold(0.0, f64::max);
                let (trusted, provenance) = evidence(&connection, &merchant.id, &query)?;
                if excluded(&query, &provenance) && !trusted {
                    continue;
                }
                if score >= 0.35 {
                    found.entry(merchant.id.clone()).or_insert(Candidate {
                        merchant,
                        score,
                        exact: false,
                        trusted,
                        provenance,
                    });
                }
            }
        }
        let mut results: Vec<_> = found.into_values().collect();
        results.sort_by(|a, b| {
            b.exact
                .cmp(&a.exact)
                .then_with(|| b.score.total_cmp(&a.score))
                .then_with(|| a.merchant.id.cmp(&b.merchant.id))
        });
        // Return all exact collisions so a truncated shortlist never appears unique.
        let count = results.iter().take_while(|r| r.exact).count().max(limit);
        results.truncate(count.min(254));
        Ok(results)
    }
}
fn validate(merchant: &Merchant) -> Result<()> {
    if merchant.id.trim().is_empty() || normalize(&merchant.name).is_empty() {
        bail!("merchant ID and name must be nonblank");
    }
    if merchant
        .country
        .as_ref()
        .is_some_and(|c| c.len() != 2 || !c.bytes().all(|b| b.is_ascii_uppercase()))
    {
        bail!("country must be a two-letter uppercase code");
    }
    if merchant.aliases.iter().any(|a| normalize(a).is_empty()) {
        bail!("aliases must be nonblank");
    }
    Ok(())
}

fn source_records(connection: &Connection, id: &str) -> Result<Vec<SourceRecord>> {
    let mut statement = connection.prepare(
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
        .query_row("SELECT data FROM manual_merchants WHERE id=?1", [id], |r| {
            r.get(0)
        })
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
    transaction.execute("INSERT INTO merchants VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET country=excluded.country,data=excluded.data", params![id,merchant.country,serde_json::to_string(&merchant)?])?;
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

fn similarity(a: &str, b: &str) -> f64 {
    let aa: HashSet<_> = a.split_whitespace().collect();
    let bb: HashSet<_> = b.split_whitespace().collect();
    let overlap = aa.intersection(&bb).count() as f64 / aa.union(&bb).count().max(1) as f64;
    let token_similarity = aa
        .iter()
        .map(|token| {
            bb.iter()
                .map(|candidate| strsim::normalized_levenshtein(token, candidate))
                .fold(0.0, f64::max)
        })
        .sum::<f64>()
        / aa.len().max(1) as f64;
    0.7 * strsim::normalized_levenshtein(a, b).max(token_similarity) + 0.3 * overlap
}

#[cfg(test)]
mod tests {
    use super::*;
    fn merchant(id: &str, name: &str, country: &str) -> Merchant {
        Merchant {
            id: id.into(),
            name: name.into(),
            country: Some(country.into()),
            website: None,
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
    fn normalization_fuzzy_retrieval_and_country_filter() {
        let db = MerchantStore::memory().unwrap();
        db.put(&merchant("a", "Julius Café", "CA")).unwrap();
        db.put(&merchant("b", "Julius Café", "US")).unwrap();
        let exact = db.search("JULIUS CAFE", Some("CA"), 10).unwrap();
        assert_eq!(exact[0].merchant.id, "a");
        assert!(exact[0].exact);
        assert_eq!(exact.len(), 1);
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
