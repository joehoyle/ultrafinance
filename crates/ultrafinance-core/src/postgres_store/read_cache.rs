//! Reuse catalog data across descriptor hypotheses and batch transactions.
//! A cheap revision lookup invalidates entries after catalog writes.
use super::*;

#[cfg(test)]
#[path = "../../tests/unit/postgres_store/fuzzy_words_tests.rs"]
mod fuzzy_words_tests;

pub(super) struct CachedProcessorFormat {
    pub format: crate::processor_formats::ProcessorFormat,
    pub regex: regex::Regex,
    pub processor: Merchant,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct SearchKey {
    pub description: String,
    pub country: Option<String>,
    pub limit: usize,
    pub entity_type: Option<String>,
}

#[derive(Default)]
pub(super) struct ReadCache {
    revision: Option<i64>,
    searches: HashMap<SearchKey, Vec<Candidate>>,
    search_order: std::collections::VecDeque<SearchKey>,
    search_rows: usize,
    processor_formats: Option<Arc<Vec<CachedProcessorFormat>>>,
    fuzzy_words: HashMap<String, Vec<String>>,
    rules: Option<Arc<Vec<(String, crate::regex_rules::Pattern)>>>,
    outlets: HashMap<String, Arc<Vec<LocationRecord>>>,
    outlet_rows: usize,
}
impl ReadCache {
    pub(super) fn refresh(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        let revision = tx
            .query_one("SELECT revision FROM catalog_revision", &[])?
            .get(0);
        if self.revision != Some(revision) {
            *self = Self {
                revision: Some(revision),
                ..Self::default()
            };
        }
        Ok(())
    }
    pub(super) fn cached_search(&self, key: &SearchKey) -> Option<Vec<Candidate>> {
        self.searches.get(key).cloned()
    }
    pub(super) fn cache_search(&mut self, key: SearchKey, candidates: Vec<Candidate>) {
        // Retain bounded query pools, including misses. No whole-request or
        // transaction descriptor decisions are cached here.
        const MAX_ROWS: usize = 8192;
        if candidates.len() > MAX_ROWS {
            return;
        }
        if let Some(old) = self.searches.remove(&key) {
            self.search_rows -= old.len();
            self.search_order.retain(|existing| existing != &key);
        }
        while self.searches.len() >= 64 || self.search_rows + candidates.len() > MAX_ROWS {
            let Some(old) = self.search_order.pop_front() else {
                break;
            };
            if let Some(rows) = self.searches.remove(&old) {
                self.search_rows -= rows.len();
            }
        }
        self.search_rows += candidates.len();
        self.search_order.push_back(key.clone());
        self.searches.insert(key, candidates);
    }

    pub(super) fn processor_formats(
        &mut self,
        tx: &mut Transaction<'_>,
    ) -> Result<Arc<Vec<CachedProcessorFormat>>> {
        self.refresh(tx)?;
        if let Some(formats) = &self.processor_formats {
            return Ok(formats.clone());
        }
        let mut formats = Vec::new();
        for format in super::processor_formats::read(tx)?
            .into_iter()
            .filter(|f| f.enabled)
        {
            let Some(row) = tx.query_opt(
                "SELECT * FROM merchants WHERE id=$1 AND counterparty_type='payment_processor'",
                &[&format.processor_id],
            )?
            else {
                continue;
            };
            formats.push(CachedProcessorFormat {
                regex: format.compile()?,
                processor: rows::merchant(&row)?,
                format,
            });
        }
        let formats = Arc::new(formats);
        self.processor_formats = Some(formats.clone());
        Ok(formats)
    }

    pub(super) fn rules(
        &mut self,
        tx: &mut Transaction<'_>,
    ) -> Result<Arc<Vec<(String, crate::regex_rules::Pattern)>>> {
        self.refresh(tx)?;
        if let Some(rules) = &self.rules {
            return Ok(rules.clone());
        }
        let rules: Arc<Vec<(String, crate::regex_rules::Pattern)>> = Arc::new(tx.query("SELECT merchant_id,transaction_pattern FROM source_records WHERE source='open-enrichment' AND coalesce(parent_id,'')='' AND transaction_pattern IS NOT NULL", &[])?
            .into_iter().filter_map(|r| crate::regex_rules::Pattern::compile(r.get(1)).map(|pattern| (r.get(0),pattern))).collect());
        self.rules = Some(rules.clone());
        Ok(rules)
    }
    pub(super) fn fuzzy_words(
        &mut self,
        tx: &mut Transaction<'_>,
        tokens: &[&str],
    ) -> Result<Vec<String>> {
        if tokens.is_empty() {
            return Ok(vec![]);
        }
        self.refresh(tx)?;
        if self.fuzzy_words.len() + tokens.len() > 2048 {
            self.fuzzy_words.clear();
        }
        let mut missing: Vec<_> = tokens
            .iter()
            .copied()
            .filter(|token| !self.fuzzy_words.contains_key(*token))
            .collect();
        missing.sort_unstable();
        missing.dedup();
        if !missing.is_empty() {
            let mut expanded: HashMap<&str, Vec<String>> = missing.iter()
                .map(|&token| (token, vec![])).collect();
            // Four results above 0.5 are also the original top four above 0.3.
            // Only tokens with fewer results need the broader, expensive scan.
            // Keep the same exact-word exclusion and similarity/lexical order.
            for threshold in ["0.5", "0.3"] {
                if missing.is_empty() { break; }
                tx.batch_execute(&format!("SET LOCAL pg_trgm.similarity_threshold='{threshold}'"))?;
                for token in &missing { expanded.get_mut(token).unwrap().clear(); }
                let rows = tx.query(
                    "SELECT query.word,fuzzy.word FROM unnest($1::text[]) query(word)
                    CROSS JOIN LATERAL (
                        SELECT word FROM merchant_search_words
                        WHERE word % query.word AND word <> query.word
                        ORDER BY similarity(word,query.word) DESC,word LIMIT 4
                    ) fuzzy",
                    &[&missing],
                )?;
                for row in rows {
                    expanded.get_mut(row.get::<_, &str>(0)).unwrap().push(row.get(1));
                }
                missing.retain(|token| expanded[token].len() < 4);
            }
            // Other trigram paths in this transaction expect the original
            // threshold. Publish cache entries only after both passes succeed.
            tx.batch_execute("SET LOCAL pg_trgm.similarity_threshold='0.3'")?;
            // Retain empty results too. Repeated descriptors/hypotheses should
            // not run the same negative fuzzy lookup until the catalog changes.
            for (token, words) in expanded {
                self.fuzzy_words.insert(token.into(), words);
            }
        }
        let mut words: Vec<_> = tokens
            .iter()
            .filter_map(|token| self.fuzzy_words.get(*token))
            .flatten()
            .cloned()
            .collect();
        words.sort_unstable();
        words.dedup();
        Ok(words)
    }
    pub(super) fn outlets(
        &mut self,
        tx: &mut Transaction<'_>,
        id: &str,
    ) -> Result<Arc<Vec<LocationRecord>>> {
        self.refresh(tx)?;
        self.load_outlets(tx, id)
    }
    pub(super) fn outlets_many(
        &mut self,
        tx: &mut Transaction<'_>,
        ids: &[String],
    ) -> Result<HashMap<String, Arc<Vec<LocationRecord>>>> {
        self.refresh(tx)?;
        let mut output = HashMap::new();
        for id in ids {
            output.insert(id.clone(), self.load_outlets(tx, id)?);
        }
        Ok(output)
    }
    fn load_outlets(
        &mut self,
        tx: &mut Transaction<'_>,
        id: &str,
    ) -> Result<Arc<Vec<LocationRecord>>> {
        if let Some(records) = self.outlets.get(id) {
            return Ok(records.clone());
        }
        let records: Vec<_> = location_rows(tx, Some(id))?
            .into_iter()
            .map(|(_, r)| r)
            .collect();
        let ids: Vec<_> = records
            .iter()
            .filter_map(|r| r.location.id.clone())
            .collect();
        // Follow redirects only for these outlets, rather than reading every
        // location redirect in the catalog on every merchant lookup.
        let redirects = tx.query("WITH RECURSIVE links AS (SELECT retired_id,location_id FROM location_redirects WHERE retired_id=ANY($1) UNION SELECT d.retired_id,d.location_id FROM location_redirects d JOIN links l ON d.retired_id=l.location_id) SELECT retired_id,location_id FROM links", &[&ids])?
            .into_iter().map(|r| (r.get(0), r.get(1))).collect();
        let records = Arc::new(crate::location_dedupe::consolidate(records, &redirects));
        // Bound retained memory, while allowing one large chain to be reused.
        // Several role/hypothesis pools can contribute more than 64 distinct
        // identities to one request. Retain that bounded working set without
        // increasing the existing 50,000-record memory budget.
        if self.outlets.len() >= 512 || self.outlet_rows + records.len() > 50_000 {
            self.outlets.clear();
            self.outlet_rows = 0;
        }
        self.outlet_rows += records.len();
        self.outlets.insert(id.into(), records.clone());
        Ok(records)
    }
}
