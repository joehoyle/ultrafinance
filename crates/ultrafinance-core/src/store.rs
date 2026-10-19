use crate::{
    Merchant,
    location::{LocationRecord, MerchantReference},
};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
#[path = "postgres_store.rs"]
mod postgres_store;

pub const LOCAL_DATABASE_URL: &str =
    "postgresql://ultrafinance@127.0.0.1:55432/ultrafinance?sslmode=disable";
pub const POSTGRES_SCHEMA_VERSION: i32 = 20;

#[derive(Debug)]
pub struct SchemaMismatch {
    pub found: i32,
    pub required: i32,
}

impl std::fmt::Display for SchemaMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PostgreSQL schema version {} is incompatible with required version {}; run `ultrafinance database init` using this release's administrator CLI before deploying",
            self.found, self.required
        )
    }
}

impl std::error::Error for SchemaMismatch {}

#[derive(Clone)]
pub struct MerchantStore(postgres_store::PostgresStore);
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

impl MerchantStore {
    pub(crate) fn persist_research(
        &self,
        request: &crate::EnrichRequest,
        merchant: &Merchant,
        new: bool,
        source: &str,
        model: &str,
        evidence: Value,
    ) -> Result<Merchant> {
        let mut merchant = merchant.clone();
        if !new {
            merchant.id = self.resolve_merchant_id(&merchant.id)?;
        }
        let saved = self
            .0
            .persist_research(request, &merchant, new, source, model, evidence)?;
        self.get(&saved.id)?
            .ok_or_else(|| anyhow::anyhow!("saved research merchant missing"))
    }

    pub fn website_logo(&self, website: &str) -> Result<Option<crate::logos::WebsiteLogo>> {
        self.0.website_logo(website)
    }
    pub fn list_website_logos(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<crate::logos::LogoPage> {
        if !(1..=1000).contains(&limit) || offset > i64::MAX as usize {
            bail!("logo limit must be 1..1000 and offset must fit an integer");
        }
        self.0.list_website_logos(limit, offset)
    }
    pub fn save_website_logo(&self, logo: &crate::logos::WebsiteLogo) -> Result<()> {
        self.0.save_website_logo(logo)
    }
    fn apply_logos<'a>(&self, merchants: impl Iterator<Item = &'a mut Merchant>) -> Result<()> {
        let pending: Vec<_> = merchants
            .filter(|m| m.logo_url.is_none())
            .filter_map(|m| {
                let website = crate::logos::normalize_website(m.website.as_deref()?).ok()?;
                Some((website, m))
            })
            .collect();
        let logos = self
            .0
            .logo_fallbacks(pending.iter().map(|(url, _)| url.clone()).collect())?;
        for (website, merchant) in pending {
            if let Some((url, source)) = logos.get(&website) {
                merchant.logo_url = Some(url.clone());
                merchant.logo_source = Some(source.clone());
            }
        }
        Ok(())
    }
    pub fn postgres(url: &str) -> Result<Self> {
        Ok(Self(postgres_store::PostgresStore::connect(url, false)?))
    }
    pub fn postgres_lazy(url: &str) -> Result<Self> {
        Ok(Self(postgres_store::PostgresStore::lazy(url)?))
    }
    pub fn initialize_postgres(url: &str) -> Result<Self> {
        Ok(Self(postgres_store::PostgresStore::connect(url, true)?))
    }
    pub fn configured(database_url: Option<&str>) -> Result<Self> {
        Self::postgres(database_url.unwrap_or(LOCAL_DATABASE_URL))
    }
    /// Isolated PostgreSQL catalog for fixtures/evaluations, dropped with the last clone.
    /// Uses the test URL or local Docker server, never the production URL implicitly.
    pub fn temporary() -> Result<Self> {
        let url = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
            .unwrap_or_else(|_| LOCAL_DATABASE_URL.into());
        Self::temporary_on(&url)
    }
    pub fn temporary_on(url: &str) -> Result<Self> {
        Ok(Self(postgres_store::PostgresStore::temporary(url)?))
    }
    pub fn temporary_url(&self) -> &str {
        self.0
            .temporary_url()
            .expect("store is not a temporary catalog")
    }
    pub fn get(&self, id: &str) -> Result<Option<Merchant>> {
        let mut merchant = self.0.get(&self.resolve_merchant_id(id)?)?;
        self.apply_logos(merchant.iter_mut())?;
        Ok(merchant)
    }
    pub fn resolutions(
        &self,
        id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::resolution::Resolution>> {
        if !(1..=1000).contains(&limit) {
            bail!("resolution limit must be 1..1000");
        }
        self.0.resolutions(id, limit)
    }
    pub fn save_resolution(&self, resolution: &crate::resolution::Resolution) -> Result<bool> {
        if resolution.verified
            && resolution
                .evidence
                .as_ref()
                .is_none_or(|s| s.trim().is_empty() || s.len() > 4096)
        {
            bail!("verified resolutions require nonblank review evidence up to 4096 bytes");
        }
        let request: crate::EnrichRequest = serde_json::from_value(resolution.context.clone())?;
        request.validate()?;
        if resolution.id != crate::resolution::key(&resolution.context) {
            bail!("invalid resolution ID");
        }
        let mut resolution = resolution.clone();
        resolution.merchant = self
            .get(&resolution.merchant.id)?
            .ok_or_else(|| anyhow::anyhow!("resolution merchant is missing"))?;
        self.0.save_resolution(&resolution)
    }
    pub fn revoke_resolution(&self, id: &str) -> Result<bool> {
        self.0.revoke_resolution(id)
    }
    /// Context-scoped mappings first, then bounded interpretation-based retrieval.
    pub fn search_request(
        &self,
        request: &crate::EnrichRequest,
        limit: usize,
    ) -> Result<Vec<Candidate>> {
        request.validate()?;
        if limit == 0 {
            return Ok(vec![]);
        }
        let context = crate::resolution::context(request);
        let id = crate::resolution::key(&context);
        let mapping = self
            .resolutions(Some(&id), 1)?
            .into_iter()
            .find(|r| r.context == context);
        let mut found = self.search(&request.description, request.country.as_deref(), limit)?;
        if let Some(mapping) = mapping
            && let Some(merchant) = self.get(&mapping.merchant.id)?
        {
            // Remembered decisions may contain legacy branch-by-branch evidence.
            // Use the current catalog contribution without rewriting audit data.
            let current = self.merchant_provenance(&merchant.id)?;
            let candidate = Candidate {
                merchant,
                score: 1.0,
                exact: mapping.verified,
                trusted: mapping.verified,
                regex_match_length: None,
                provenance: if current.is_empty() {
                    mapping.provenance
                } else {
                    current
                },
                resolution_id: Some(id),
                pending_import: false,
                interpretation_evidence: vec![],
                business_evidence: serde_json::Value::Null,
            };
            if mapping.verified {
                return Ok(vec![candidate]);
            }
            if !found
                .iter()
                .any(|c| c.merchant.id == candidate.merchant.id && c.exact && c.trusted)
            {
                found.retain(|c| c.merchant.id != candidate.merchant.id);
                found.insert(0, candidate);
            }
        }
        let interpretation = crate::interpretation::interpret(request);
        let mut searched = std::collections::HashSet::from([normalize(&request.description)]);
        // Never upgrade a partial-name hypothesis to an exact or trusted alias.
        for (index, hypothesis) in interpretation.hypotheses.iter().enumerate() {
            if !searched.insert(normalize(&hypothesis.merchant_text)) {
                continue;
            }
            for mut candidate in
                self.search(&hypothesis.merchant_text, request.country.as_deref(), limit)?
            {
                candidate.exact = false;
                candidate.trusted = false;
                candidate.score = candidate.score.min(0.9 - 0.05 * index as f64);
                if excluded(&normalize(&request.description), &candidate.provenance) {
                    continue;
                }
                if let Some(existing) = found
                    .iter_mut()
                    .find(|c| c.merchant.id == candidate.merchant.id)
                {
                    existing.score = existing.score.max(candidate.score);
                } else {
                    found.push(candidate);
                }
            }
        }
        for candidate in &mut found {
            let needs_outlets = interpretation.hypotheses.iter().any(|hypothesis| {
                (hypothesis.possible_location.is_some() || hypothesis.location_hint.is_some())
                    && crate::interpretation::supporting_name(
                        &candidate.merchant,
                        &hypothesis.merchant_text,
                    )
                    .is_some()
            });
            let outlets = if needs_outlets {
                self.cached_locations(&candidate.merchant.id)?
            } else {
                std::sync::Arc::new(vec![])
            };
            candidate.interpretation_evidence = crate::interpretation::catalog_support(
                &interpretation,
                &candidate.merchant,
                &outlets,
                request
                    .country
                    .as_deref()
                    .or_else(|| request.location.as_ref().and_then(|l| l.country.as_deref())),
            );
            // Evidence affects retrieval priority, never automatic trust.
            if candidate.resolution_id.is_none() {
                for support in &candidate.interpretation_evidence {
                    if support.name_exact && support.possible_location.is_none() {
                        candidate.score = candidate.score.max(0.96);
                    } else if support.outlet.is_some() {
                        candidate.score = candidate.score.max(0.91);
                    }
                }
            }
        }
        let mut ranked = rank_candidates(
            found
                .into_iter()
                .map(|c| (c.merchant.id.clone(), c))
                .collect(),
            limit,
            request.country.as_deref(),
        );
        // Hydrate only the final shortlist; outlet reads are cached by catalog
        // revision, while relevance is computed for this transaction alone.
        for candidate in &mut ranked {
            let outlets = self.cached_locations(&candidate.merchant.id)?;
            candidate.business_evidence = crate::business::outlet_summary(request, &outlets);
        }
        Ok(ranked)
    }

    pub(crate) fn write_logs(
        &self,
        entries: Vec<(String, String, Value)>,
        finished: bool,
    ) -> Result<()> {
        let records = entries
            .into_iter()
            .map(|(id, batch, data)| {
                let status = if finished {
                    data["status"].as_str().unwrap()
                } else {
                    "started"
                }
                .to_owned();
                let merchant = data["response"]["counterparties"][0]["id"]
                    .as_str()
                    .or_else(|| data["response"]["merchant"]["data"]["id"].as_str())
                    .map(str::to_owned);
                Ok((id, batch, status, merchant, serde_json::to_string(&data)?))
            })
            .collect::<Result<Vec<_>>>()?;
        self.0.write_logs(records)
    }
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
        self.0.logs(status, merchant, limit, offset)
    }
    pub fn import_locations(&self, records: &[LocationRecord]) -> Result<()> {
        self.0.import_locations(records)
    }
    pub fn locations(&self, id: &str) -> Result<Vec<LocationRecord>> {
        self.0.locations(&self.resolve_merchant_id(id)?)
    }
    pub fn location_sources(&self, id: &str) -> Result<Vec<LocationRecord>> {
        self.0.location_sources(&self.resolve_merchant_id(id)?)
    }
    pub(crate) fn cached_locations(&self, id: &str) -> Result<std::sync::Arc<Vec<LocationRecord>>> {
        self.0.cached_locations(&self.resolve_merchant_id(id)?)
    }
    pub fn dedupe_locations(
        &self,
        id: Option<&str>,
        dry_run: bool,
    ) -> Result<crate::location_dedupe::Report> {
        let id = id.map(|id| self.resolve_merchant_id(id)).transpose()?;
        self.0.dedupe_locations(id.as_deref(), dry_run)
    }
    pub fn put(&self, m: &Merchant) -> Result<()> {
        self.0.put(m)
    }
    /// Low-level source persistence. User-facing imports should use
    /// `dedupe::import` to reconcile merchant identity atomically.
    pub fn import(&self, r: &[SourceRecord]) -> Result<()> {
        self.import_delta(r).map(|_| ())
    }
    pub fn import_delta(&self, r: &[SourceRecord]) -> Result<ImportDelta> {
        self.0.import(r)
    }
    pub(crate) fn reconcile_import(
        &self,
        records: Vec<SourceRecord>,
        dry_run: bool,
        chunk_size: usize,
    ) -> Result<crate::dedupe::ImportReport> {
        self.0.reconcile_import(records, dry_run, chunk_size)
    }
    pub(crate) fn reconcile_generated<F>(
        &self,
        total: usize,
        dry_run: bool,
        chunk_size: usize,
        produce: F,
    ) -> Result<crate::dedupe::ImportReport>
    where
        F: FnOnce(&mut dyn FnMut(Vec<SourceRecord>) -> Result<()>) -> Result<()> + Send + 'static,
    {
        self.0
            .reconcile_generated(total, dry_run, chunk_size, produce)
    }
    pub(crate) fn initial_foursquare_load<F>(
        &self,
        total: usize,
        dry_run: bool,
        chunk_size: usize,
        produce: F,
    ) -> Result<crate::dedupe::ImportReport>
    where
        F: FnOnce(&mut dyn FnMut(Vec<SourceRecord>) -> Result<()>) -> Result<()> + Send + 'static,
    {
        self.0
            .initial_foursquare_load(total, dry_run, chunk_size, produce)
    }
    pub(crate) fn import_database_identity(&self) -> Result<String> {
        self.0.import_database_identity()
    }
    pub(crate) fn reconcile_file(
        &self,
        path: std::path::PathBuf,
        limit: Option<u32>,
        dry_run: bool,
        chunk_size: usize,
    ) -> Result<(crate::dedupe::ImportReport, usize, usize)> {
        self.0.reconcile_file(path, limit, dry_run, chunk_size)
    }
    /// Consolidated merchant evidence; upstream place mappings remain available
    /// through source_records and physical branch details through locations.
    pub fn merchant_provenance(&self, id: &str) -> Result<Vec<SourceRecord>> {
        self.0.merchant_provenance(&self.resolve_merchant_id(id)?)
    }
    /// Browse original imported evidence independently of candidate retrieval.
    pub fn source_records(
        &self,
        source: &str,
        external_id: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<serde_json::Value>> {
        if !(1..=1000).contains(&limit) {
            bail!("source record limit must be 1..1000");
        }
        self.0.source_records(source, external_id, limit, offset)
    }
    pub fn link(&self, source: &str, external: &str, target: &str) -> Result<()> {
        self.0
            .link(source, external, &self.resolve_merchant_id(target)?)
    }
    pub fn resolve_source(&self, s: &str, id: &str) -> Result<Option<String>> {
        self.0.resolve_source(s, id)
    }
    pub(crate) fn dedupe_candidates(
        &self,
        max_pairs: usize,
    ) -> Result<(crate::dedupe::Snapshot, Vec<(usize, usize)>)> {
        self.0.dedupe_candidates(max_pairs)
    }
    pub fn apply_dedupe(
        &self,
        expected: &crate::dedupe::Snapshot,
        groups: &[Vec<String>],
    ) -> Result<String> {
        self.0.apply_dedupe(expected, groups)
    }
    pub fn resolve_merchant_id(&self, id: &str) -> Result<String> {
        self.0.resolve_merchant_id(id)
    }
    pub(crate) fn apply_dedupe_events(
        &self,
        revision: i64,
        groups: Vec<Vec<String>>,
    ) -> Result<String> {
        self.0.apply_dedupe_events(revision, groups)
    }
    pub fn stats(&self) -> Result<MerchantStats> {
        self.0.stats()
    }
    pub fn list(&self, market: Option<&str>, limit: usize, offset: usize) -> Result<MerchantPage> {
        self.list_typed(market, limit, offset, None)
    }
    pub fn list_typed(
        &self,
        market: Option<&str>,
        limit: usize,
        offset: usize,
        entity_type: Option<crate::CounterpartyType>,
    ) -> Result<MerchantPage> {
        let mut page = self.0.list(market, limit, offset, entity_type)?;
        self.apply_logos(page.merchants.iter_mut())?;
        Ok(page)
    }
    pub fn search(
        &self,
        description: &str,
        country: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Candidate>> {
        let mut candidates = self.0.search(description, country, limit)?;
        self.apply_logos(candidates.iter_mut().map(|c| &mut c.merchant))?;
        Ok(candidates)
    }
    pub fn search_typed(
        &self,
        description: &str,
        country: Option<&str>,
        limit: usize,
        entity_type: Option<crate::CounterpartyType>,
    ) -> Result<Vec<Candidate>> {
        let mut candidates = self
            .0
            .search_typed(description, country, limit, entity_type)?;
        self.apply_logos(candidates.iter_mut().map(|c| &mut c.merchant))?;
        Ok(candidates)
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    #[serde(skip_serializing_if = "Value::is_null")]
    pub business_evidence: Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub interpretation_evidence: Vec<crate::interpretation::CatalogSupport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution_id: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pending_import: bool,
    pub merchant: Merchant,
    pub score: f64,
    pub exact: bool,
    /// Number of descriptor characters matched by an Open Enrichment rule.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regex_match_length: Option<usize>,
    pub trusted: bool,
    /// Merchant-level source contributions, not one document per linked outlet.
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
    pub without_markets: usize,
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

// Aggregate queries for PostgreSQL catalog statistics.
const STATS_TOTALS: &str = "SELECT (SELECT COUNT(*) FROM merchants), (SELECT COUNT(*) FROM manual_merchants), (SELECT COUNT(*) FROM merchants m WHERE NOT EXISTS (SELECT 1 FROM source_identities s WHERE s.merchant_id=m.id))";
const STATS_SOURCES: &str = "SELECT source,COUNT(DISTINCT merchant_id),COUNT(*) FROM source_identities GROUP BY source ORDER BY COUNT(DISTINCT merchant_id) DESC,source";

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
    /// Upstream input payload. Persisted records expose only matching fields.
    pub raw: Value,
}

impl SourceRecord {
    /// Only raw fields that affect matching are retained in the catalog.
    pub(crate) fn matching_raw(&self) -> Value {
        let mut raw = serde_json::Map::new();
        for key in ["countryHints", "negativeAliases"] {
            let values: Vec<Value> = self.raw[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(|v| Value::String(v.into())))
                .collect();
            if !values.is_empty() {
                raw.insert(key.into(), Value::Array(values));
            }
        }
        for key in ["transaction_text_regexp", "parent_id"] {
            if let Some(value) = self.raw[key].as_str() {
                raw.insert(key.into(), Value::String(value.into()));
            }
        }
        Value::Object(raw)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ImportDelta {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
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

fn excluded(query: &str, records: &[SourceRecord]) -> bool {
    excluded_aliases(
        query,
        records.iter().flat_map(|r| {
            r.raw["negativeAliases"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
        }),
    )
}
fn excluded_aliases<'a>(query: &str, mut aliases: impl Iterator<Item = &'a str>) -> bool {
    // Imported negative aliases are whole phrases, never arbitrary substrings.
    let query = format!(" {query} ");
    aliases.any(|a| {
        let a = normalize(a);
        !a.is_empty() && query.contains(&format!(" {a} "))
    })
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

#[cfg(test)]
#[path = "../tests/unit/postgres_tests.rs"]
mod postgres_tests;
#[cfg(test)]
#[path = "../tests/unit/store/tests.rs"]
mod tests;

#[cfg(test)]
include!("../tests/unit/store/support.rs");
