//! Reuse catalog data across descriptor hypotheses and batch transactions.
//! A cheap revision lookup invalidates entries after catalog writes.
use super::*;

#[derive(Default)]
pub(super) struct ReadCache {
    revision: Option<i64>,
    fuzzy_words: HashMap<String, Vec<String>>,
    rules: Option<Arc<Vec<(String, crate::regex_rules::Pattern)>>>,
    outlets: HashMap<String, Arc<Vec<LocationRecord>>>,
    outlet_rows: usize,
}
impl ReadCache {
    fn refresh(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
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
            tx.batch_execute("SET LOCAL pg_trgm.similarity_threshold='0.3'")?;
            let rows = tx.query(
                "SELECT query.word,fuzzy.word FROM unnest($1::text[]) query(word)
                CROSS JOIN LATERAL (
                    SELECT word FROM merchant_search_words
                    WHERE word % query.word AND word <> query.word
                    ORDER BY similarity(word,query.word) DESC,word LIMIT 4
                ) fuzzy",
                &[&missing],
            )?;
            // Retain empty results too. Repeated descriptors/hypotheses should
            // not run the same negative fuzzy lookup until the catalog changes.
            for token in missing {
                self.fuzzy_words.insert(token.into(), vec![]);
            }
            for row in rows {
                self.fuzzy_words
                    .get_mut(row.get::<_, &str>(0))
                    .unwrap()
                    .push(row.get(1));
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
        if self.outlets.len() >= 64 || self.outlet_rows + records.len() > 50_000 {
            self.outlets.clear();
            self.outlet_rows = 0;
        }
        self.outlet_rows += records.len();
        self.outlets.insert(id.into(), records.clone());
        Ok(records)
    }
}
