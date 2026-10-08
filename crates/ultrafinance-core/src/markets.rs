//! Known operating coverage is positive evidence, never an exhaustive restriction.
use crate::{Merchant, location::LocationRecord, store::SourceRecord};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MarketKind {
    Declaration,
    CountryHint,
    DatasetRegion,
    Outlet,
}

/// Qualitative strength of the supplied evidence, not a match probability.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MarketConfidence {
    High,
    Medium,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct MarketEvidence {
    pub country: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    pub kind: MarketKind,
    pub confidence: MarketConfidence,
}

pub(crate) fn valid_country(country: &str) -> bool {
    country.len() == 2 && country.bytes().all(|b| b.is_ascii_uppercase())
}

/// Adapter regions describe where transactions in this dataset were observed.
pub(crate) fn dataset_region(record: &SourceRecord) -> Option<String> {
    if record.source != "open-enrichment" {
        return None;
    }
    let region = record.version.as_deref()?.split_once(':')?.0;
    (!region.is_empty()).then(|| region.to_owned())
}
pub(crate) fn source_region(record: &SourceRecord) -> Option<String> {
    let region = dataset_region(record)?.to_ascii_uppercase();
    valid_country(&region).then_some(region)
}

#[derive(Default)]
pub(crate) struct MarketIndex {
    evidence: HashMap<String, BTreeSet<MarketEvidence>>,
    regions: BTreeMap<(String, String), (BTreeSet<String>, usize)>,
}
impl MarketIndex {
    fn add(
        &mut self,
        id: &str,
        country: &str,
        source: &str,
        external_id: Option<&str>,
        kind: MarketKind,
        confidence: MarketConfidence,
    ) {
        if valid_country(country) {
            self.evidence
                .entry(id.into())
                .or_default()
                .insert(MarketEvidence {
                    country: country.into(),
                    source: source.into(),
                    external_id: external_id.map(str::to_owned),
                    kind,
                    confidence,
                });
        }
    }
    pub fn declaration(
        &mut self,
        id: &str,
        merchant: &Merchant,
        source: &str,
        external_id: Option<&str>,
    ) {
        // Preserve supplied provenance and confidence rather than upgrading it.
        for evidence in &merchant.market_evidence {
            if valid_country(&evidence.country) {
                self.evidence
                    .entry(id.into())
                    .or_default()
                    .insert(evidence.clone());
            }
        }
        for country in merchant.markets.iter() {
            if !merchant
                .market_evidence
                .iter()
                .any(|e| &e.country == country)
            {
                self.add(
                    id,
                    country,
                    source,
                    external_id,
                    MarketKind::Declaration,
                    MarketConfidence::High,
                );
            }
        }
    }
    pub fn source(&mut self, id: &str, record: &SourceRecord) {
        self.declaration(
            id,
            &record.merchant,
            &record.source,
            Some(&record.external_id),
        );
        if let Some(hints) = record.raw["countryHints"].as_array() {
            for country in hints.iter().filter_map(|v| v.as_str()) {
                self.add(
                    id,
                    country,
                    &record.source,
                    Some(&record.external_id),
                    MarketKind::CountryHint,
                    MarketConfidence::Medium,
                );
            }
        }
        if let Some(region) = dataset_region(record) {
            let entry = self
                .regions
                .entry((record.source.clone(), region.clone()))
                .or_default();
            entry.0.insert(id.into());
            entry.1 += 1;
        }
        if let Some(region) = source_region(record) {
            self.add(
                id,
                &region,
                &record.source,
                Some(&record.external_id),
                MarketKind::DatasetRegion,
                MarketConfidence::Medium,
            );
        }
    }
    pub fn outlet(&mut self, id: &str, record: &LocationRecord) {
        if let Some(country) = &record.location.country {
            self.add(
                id,
                country,
                &record.source,
                Some(&record.external_id),
                MarketKind::Outlet,
                MarketConfidence::High,
            );
        }
    }
    pub fn region_stats(&self) -> Vec<crate::store::MerchantRegionStats> {
        self.regions
            .iter()
            .map(
                |((source, region), (merchants, records))| crate::store::MerchantRegionStats {
                    source: source.clone(),
                    region: region.clone(),
                    merchants: merchants.len(),
                    records: *records,
                },
            )
            .collect()
    }
    pub fn hydrate(&self, mut merchant: Merchant) -> Merchant {
        let evidence = self.evidence.get(&merchant.id).cloned().unwrap_or_default();
        merchant.markets = evidence
            .iter()
            .map(|e| e.country.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        merchant.market_evidence = evidence.into_iter().collect();
        merchant
    }
}

pub(crate) fn market_counts(
    merchants: &[Merchant],
) -> (usize, Vec<crate::store::MerchantMarketStats>) {
    let mut counts = BTreeMap::<String, usize>::new();
    let mut unknown = 0;
    for merchant in merchants {
        if merchant.markets.is_empty() {
            unknown += 1;
        }
        for market in &merchant.markets {
            *counts.entry(market.clone()).or_default() += 1;
        }
    }
    let mut rows: Vec<_> = counts
        .into_iter()
        .map(|(market, merchants)| crate::store::MerchantMarketStats { market, merchants })
        .collect();
    rows.sort_by(|a, b| b.merchants.cmp(&a.merchants).then(a.market.cmp(&b.market)));
    (unknown, rows)
}

pub(crate) fn paginate(
    mut merchants: Vec<Merchant>,
    market: Option<&str>,
    limit: usize,
    offset: usize,
) -> crate::store::MerchantPage {
    if let Some(market) = market {
        merchants.retain(|m| m.markets.iter().any(|c| c == market));
    }
    merchants.sort_by_cached_key(|m| (m.name.to_lowercase(), m.id.clone()));
    let total = merchants.len();
    crate::store::MerchantPage {
        merchants: merchants.into_iter().skip(offset).take(limit).collect(),
        total,
        limit,
        offset,
    }
}

/// One-time storage migration, not an accepted merchant input format.
pub(crate) fn migrate_country(data: &str, source_record: bool) -> anyhow::Result<String> {
    let mut value: serde_json::Value = serde_json::from_str(data)?;
    let merchant = if source_record {
        &mut value["merchant"]
    } else {
        &mut value
    };
    let object = merchant
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("stored merchant must be an object"))?;
    if let Some(country) = object.remove("country").filter(|c| !c.is_null()) {
        let markets = object
            .entry("markets")
            .or_insert_with(|| serde_json::json!([]));
        let countries = markets
            .as_array_mut()
            .ok_or_else(|| anyhow::anyhow!("stored markets must be an array"))?;
        if !countries.contains(&country) {
            countries.push(country);
        }
    }
    if source_record {
        let record: SourceRecord = serde_json::from_value(value)?;
        super::store::validate(&record.merchant)?;
        Ok(serde_json::to_string(&record)?)
    } else {
        let merchant: Merchant = serde_json::from_value(value)?;
        super::store::validate(&merchant)?;
        Ok(serde_json::to_string(&merchant)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{import, store::MerchantStore};
    use serde_json::json;

    #[test]
    fn market_evidence_combines_sources_outlets_and_manual_declarations() -> anyhow::Result<()> {
        let store = MerchantStore::memory()?;
        let studio = r#"{"schemaVersion":"1.1.0","merchants":[{"id":"brand","canonicalName":"Brand","countryHints":["US","CA","CA"]}]}"#;
        let mut records = import::merchant_studio(studio)?;
        let mut regional = records[0].clone();
        regional.source = "open-enrichment".into();
        regional.version = Some("au:snapshot".into());
        regional.raw = json!({});
        store.import(&records)?;
        store.import(&[regional.clone()])?;
        let id = store.resolve_source("merchant-studio", "brand")?.unwrap();
        store.link("open-enrichment", "brand", &id)?;
        store.put(&serde_json::from_value(
            json!({"id":id,"name":"Brand","markets":["GB","GB"]}),
        )?)?;
        let mut outlets: Vec<LocationRecord> = serde_json::from_str(include_str!(
            "../../../data/locations/open-enrichment-au.json"
        ))?;
        outlets.truncate(1);
        outlets[0].merchant = crate::location::MerchantReference::Source {
            source: "open-enrichment".into(),
            external_id: "brand".into(),
        };
        outlets[0].location.country = Some("NZ".into());
        store.import_locations(&outlets)?;
        let merchant = store.list(None, 10, 0)?.merchants.remove(0);
        assert_eq!(merchant.markets, ["AU", "CA", "GB", "NZ", "US"]);
        assert_eq!(merchant.market_evidence.len(), 5);
        let au = merchant
            .market_evidence
            .iter()
            .find(|e| e.country == "AU")
            .unwrap();
        assert_eq!(au.kind, MarketKind::DatasetRegion);
        assert_eq!(au.confidence, MarketConfidence::Medium);
        assert_eq!(au.external_id.as_deref(), Some("brand"));
        let nz = merchant
            .market_evidence
            .iter()
            .find(|e| e.country == "NZ")
            .unwrap();
        assert_eq!(nz.kind, MarketKind::Outlet);
        assert_eq!(nz.confidence, MarketConfidence::High);
        assert_eq!(store.list(Some("CA"), 10, 0)?.total, 1);
        assert_eq!(store.list(Some("DE"), 10, 0)?.total, 0);
        let stats = store.stats()?;
        assert_eq!(stats.total, 1);
        assert_eq!(stats.by_market.len(), 5);
        assert_eq!(stats.by_source_region[0].region, "au");
        assert_eq!(stats.without_market_evidence, 0);
        // Refresh removes obsolete source hints, while manual and outlet evidence survive.
        records[0].raw = json!({"countryHints":["CA"]});
        regional.version = Some("global:snapshot".into());
        store.import(&records)?;
        store.import(&[regional])?;
        let merchant = store.list(None, 10, 0)?.merchants.remove(0);
        assert_eq!(merchant.markets, ["CA", "GB", "NZ"]);
        assert_eq!(store.stats()?.by_source_region[0].region, "global");
        // Relinking the source moves its outlet-derived coverage immediately.
        store.put(&serde_json::from_value(
            json!({"id":"other","name":"Other"}),
        )?)?;
        store.link("open-enrichment", "brand", "other")?;
        let original = store.search("Brand", None, 10)?.remove(0).merchant;
        assert_eq!(original.markets, ["CA", "GB"]);
        assert_eq!(store.list(Some("NZ"), 10, 0)?.merchants[0].id, "other");
        Ok(())
    }

    #[test]
    fn absent_markets_never_exclude_and_collisions_remain_ambiguous() -> anyhow::Result<()> {
        let store = MerchantStore::memory()?;
        for merchant in [
            json!({"id":"a","name":"Same Brand","markets":["US"]}),
            json!({"id":"b","name":"Same Brand","markets":["CA"]}),
            json!({"id":"c","name":"Same Brand"}),
        ] {
            store.put(&serde_json::from_value(merchant)?)?;
        }
        let candidates = store.search("Same Brand", Some("CA"), 1)?;
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].merchant.id, "b");
        assert_eq!(store.search("Same Brand", Some("DE"), 1)?.len(), 3);
        assert_eq!(store.stats()?.without_market_evidence, 1);
        Ok(())
    }

    #[test]
    fn publisher_geography_is_not_coverage_and_supplied_confidence_survives() -> anyhow::Result<()>
    {
        let mut records = import::catalog(
            r#"[{"id":"brand","name":"Brand","markets":["CA"],"market_evidence":[{"country":"CA","source":"reviewed-dataset","external_id":"x","kind":"country_hint","confidence":"medium"}]}]"#,
            "custom",
        )?;
        records[0].version = Some("us:snapshot".into());
        records[0].raw = json!({"publisherCountry":"US","countryHints":["NZ","invalid"]});
        let store = MerchantStore::memory()?;
        store.import(&records)?;
        let merchant = store.list(None, 10, 0)?.merchants.remove(0);
        assert_eq!(merchant.markets, ["CA", "NZ"]);
        let evidence = merchant
            .market_evidence
            .iter()
            .find(|e| e.country == "CA")
            .unwrap();
        assert_eq!(evidence.confidence, MarketConfidence::Medium);
        assert_eq!(evidence.source, "reviewed-dataset");
        assert!(store.stats()?.by_source_region.is_empty());
        assert!(
            serde_json::from_value::<Merchant>(json!({"id":"old","name":"Old","country":"CA"}))
                .is_err()
        );
        assert!(
            store
                .put(&serde_json::from_value(
                    json!({"id":"bad","name":"Bad","markets":["ca"]})
                )?)
                .is_err()
        );
        Ok(())
    }
}

#[cfg(test)]
mod migration_tests {
    use crate::{import, store::MerchantStore};
    use rusqlite::{Connection, params};
    use serde_json::{Value, json};
    #[test]
    fn sqlite_migrates_country_once_and_rolls_back_invalid_data() -> anyhow::Result<()> {
        let directory = std::env::temp_dir().join(format!(
            "ultrafinance-market-migration-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory)?;
        for invalid in [false, true] {
            let path = directory.join(format!("{invalid}.sqlite"));
            let store = MerchantStore::open(&path)?;
            store.import(&import::catalog(
                r#"[{"id":"brand","name":"Brand","markets":["CA"]}]"#,
                "test",
            )?)?;
            let id = store.resolve_source("test", "brand")?.unwrap();
            store.put(&serde_json::from_value(
                json!({"id":id,"name":"Corrected","markets":["CA"]}),
            )?)?;
            let original = store.fingerprint()?;
            drop(store);
            let connection = Connection::open(&path)?;
            connection.execute_batch(
                "ALTER TABLE merchants ADD COLUMN country TEXT; PRAGMA user_version=1;",
            )?;
            for table in ["merchants", "manual_merchants", "source_records"] {
                let rows: Vec<(i64, String)> = connection
                    .prepare(&format!("SELECT rowid,data FROM {table}"))?
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?;
                for (rowid, data) in rows {
                    let mut data: Value = serde_json::from_str(&data)?;
                    let merchant = if table == "source_records" {
                        &mut data["merchant"]
                    } else {
                        &mut data
                    };
                    let object = merchant.as_object_mut().unwrap();
                    object.remove("markets");
                    object.insert(
                        "country".into(),
                        json!(if invalid && table == "source_records" {
                            "ca"
                        } else {
                            "CA"
                        }),
                    );
                    connection.execute(
                        &format!("UPDATE {table} SET data=?1 WHERE rowid=?2"),
                        params![data.to_string(), rowid],
                    )?;
                }
            }
            drop(connection);
            let upgraded = MerchantStore::open(&path);
            if invalid {
                assert!(upgraded.is_err());
                let connection = Connection::open(&path)?;
                assert_eq!(
                    connection.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                    1
                );
                let data: String =
                    connection.query_row("SELECT data FROM manual_merchants", [], |r| r.get(0))?;
                assert_eq!(serde_json::from_str::<Value>(&data)?["country"], "CA");
                assert!(connection.prepare("SELECT country FROM merchants").is_ok());
            } else {
                let store = upgraded?;
                assert_eq!(store.fingerprint()?, original);
                assert_eq!(store.resolve_source("test", "brand")?, Some(id.clone()));
                let merchant = store.list(Some("CA"), 10, 0)?.merchants.remove(0);
                assert_eq!(merchant.id, id);
                assert_eq!(merchant.name, "Corrected");
                assert_eq!(merchant.markets, ["CA"]);
                assert!(serde_json::to_value(&merchant)?.get("country").is_none());
                drop(store);
                let reopened = MerchantStore::open(&path)?;
                assert_eq!(reopened.fingerprint()?, original);
                let connection = Connection::open(&path)?;
                assert_eq!(
                    connection.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                    3
                );
                assert!(connection.prepare("SELECT country FROM merchants").is_err());
            }
        }
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }
}
