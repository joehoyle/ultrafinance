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
        self.0.get(&self.resolve_merchant_id(id)?)
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
            let candidate = Candidate {
                merchant,
                score: 1.0,
                exact: mapping.verified,
                trusted: mapping.verified,
                regex_match_length: None,
                provenance: mapping.provenance,
                resolution_id: Some(id),
                pending_import: false,
                interpretation_evidence: vec![],
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
        // Never upgrade a partial-name hypothesis to an exact or trusted alias.
        for (index, hypothesis) in crate::interpretation::interpret(request)
            .hypotheses
            .into_iter()
            .enumerate()
        {
            if normalize(&hypothesis.merchant_text) == normalize(&request.description) {
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
        let interpretation = crate::interpretation::interpret(request);
        for candidate in &mut found {
            let needs_outlets = interpretation.hypotheses.iter().any(|hypothesis| {
                hypothesis.possible_location.is_some()
                    && std::iter::once(&candidate.merchant.name)
                        .chain(&candidate.merchant.aliases)
                        .any(|name| normalize(name) == normalize(&hypothesis.merchant_text))
            });
            let outlets = if needs_outlets {
                self.locations(&candidate.merchant.id)?
            } else {
                vec![]
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
                    if support.possible_location.is_none() {
                        candidate.score = candidate.score.max(0.96);
                    } else if support.outlet.is_some() {
                        candidate.score = candidate.score.max(0.91);
                    }
                }
            }
        }
        Ok(rank_candidates(
            found
                .into_iter()
                .map(|c| (c.merchant.id.clone(), c))
                .collect(),
            limit,
            request.country.as_deref(),
        ))
    }

    pub(crate) fn write_log(
        &self,
        id: &str,
        batch: &str,
        status: &str,
        merchant: Option<&str>,
        data: &Value,
    ) -> Result<()> {
        self.0
            .write_log(id, batch, status, merchant, &serde_json::to_string(data)?)
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
    pub fn put(&self, m: &Merchant) -> Result<()> {
        self.0.put(m)
    }
    pub fn import(&self, r: &[SourceRecord]) -> Result<()> {
        self.0.import(r)
    }
    pub fn link(&self, source: &str, external: &str, target: &str) -> Result<()> {
        self.0
            .link(source, external, &self.resolve_merchant_id(target)?)
    }
    pub fn resolve_source(&self, s: &str, id: &str) -> Result<Option<String>> {
        self.0.resolve_source(s, id)
    }
    pub fn dedupe_snapshot(&self) -> Result<crate::dedupe::Snapshot> {
        self.0.dedupe_snapshot()
    }
    pub fn apply_dedupe(
        &self,
        expected: &crate::dedupe::Snapshot,
        groups: &[Vec<String>],
        audit: &Value,
    ) -> Result<String> {
        self.0.apply_dedupe(expected, groups, audit)
    }
    pub fn resolve_merchant_id(&self, id: &str) -> Result<String> {
        self.0.resolve_merchant_id(id)
    }
    pub fn fingerprint(&self) -> Result<String> {
        self.0.fingerprint()
    }
    pub fn stats(&self) -> Result<MerchantStats> {
        self.0.stats()
    }
    pub fn list(&self, market: Option<&str>, limit: usize, offset: usize) -> Result<MerchantPage> {
        self.0.list(market, limit, offset)
    }
    pub fn search(
        &self,
        description: &str,
        country: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Candidate>> {
        self.0.search(description, country, limit)
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
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

// Aggregate queries for PostgreSQL catalog statistics.
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
        regex_search_scenarios(&MerchantStore::temporary()?)
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
    #[test]
    fn interpretation_uses_catalog_city_without_discarding_full_name() -> Result<()> {
        let store = MerchantStore::temporary()?;
        store.put(&merchant("short", "Julius Cafe", "CA"))?;
        store.put(&merchant("full", "Julius Cafe Bromont", "CA"))?;
        let request: crate::EnrichRequest = serde_json::from_value(serde_json::json!({
            "description":"SQ* JULIUS CAFE BROMONT 00482", "country":"CA"
        }))?;
        let candidates = store.search_request(&request, 10)?;
        assert_eq!(candidates[0].merchant.id, "full");
        let short = candidates
            .iter()
            .find(|c| c.merchant.id == "short")
            .unwrap();
        assert!(
            short
                .interpretation_evidence
                .iter()
                .any(|e| e.possible_location.as_deref() == Some("BROMONT") && e.outlet.is_none())
        );
        let outlet: LocationRecord = serde_json::from_value(serde_json::json!({
            "source":"reviewed-outlets", "external_id":"julius-bromont", "aliases":["Julius Cafe Bromont"],
            "merchant":{"merchant_id":"short"},
            "location":{"id":null,"precision":"outlet", "address":"123 Test Street",
                "city":"Bromont","region":"QC","postal_code":null,"country":"CA","store_number":null},
            "attribution":"Test fixture", "license":"test", "url":"https://example.com/outlet"
        }))?;
        store.import_locations(&[outlet])?;
        let candidates = store.search_request(&request, 10)?;
        assert_eq!(candidates[0].merchant.id, "full");
        let short = candidates
            .iter()
            .find(|c| c.merchant.id == "short")
            .unwrap();
        assert!(!short.exact && !short.trusted);
        let evidence = short
            .interpretation_evidence
            .iter()
            .find_map(|e| e.outlet.as_ref())
            .unwrap();
        assert_eq!(evidence.city, "Bromont");
        assert_eq!(evidence.external_id, "julius-bromont");
        let mut other_country = request.clone();
        other_country.country = Some("US".into());
        assert!(
            store
                .search_request(&other_country, 10)?
                .iter()
                .flat_map(|c| &c.interpretation_evidence)
                .all(|e| e.outlet.is_none())
        );
        assert!(store.get("short")?.unwrap().aliases.is_empty());
        Ok(())
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
        let db = MerchantStore::temporary().unwrap();
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
        let db = MerchantStore::temporary().unwrap();
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
        let db = MerchantStore::temporary().unwrap();
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
        let db = MerchantStore::temporary().unwrap();
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
        let db = MerchantStore::temporary().unwrap();
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
