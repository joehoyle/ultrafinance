//! Known operating coverage is positive evidence, never an exhaustive restriction.
use crate::{Merchant, store::SourceRecord};
use std::collections::{BTreeSet, HashMap};

pub(crate) fn valid_country(country: &str) -> bool {
    country.len() == 2 && country.bytes().all(|b| b.is_ascii_uppercase())
}

/// Adapter regions describe where transactions in this dataset were observed.
pub(crate) fn dataset_region(record: &SourceRecord) -> Option<String> {
    if record.source != "open-enrichment" {
        return None;
    }
    let version = record.version.as_deref()?;
    // Prepared bundles use fnv1a64:HASH:REGION:EXAMPLES; direct imports use REGION:VERSION.
    let region = if version.starts_with("fnv1a64:") {
        version.split(':').nth(2)?
    } else {
        version.split_once(':')?.0
    };
    (!region.is_empty()).then(|| region.to_owned())
}
#[cfg(test)]
pub(crate) fn source_region(record: &SourceRecord) -> Option<String> {
    let region = dataset_region(record)?.to_ascii_uppercase();
    valid_country(&region).then_some(region)
}

/// Compact country sets assembled from merchant and source inputs.
#[derive(Default)]
pub(crate) struct MarketCountries {
    countries: HashMap<String, BTreeSet<String>>,
}
impl MarketCountries {
    fn add(&mut self, id: &str, country: &str) {
        if valid_country(country) {
            self.countries
                .entry(id.into())
                .or_default()
                .insert(country.into());
        }
    }
    pub fn declaration(&mut self, id: &str, merchant: &Merchant) {
        for country in &merchant.markets {
            self.add(id, country);
        }
    }
    pub(crate) fn source_fields(
        &mut self,
        id: &str,
        merchant: &Merchant,
        hints: &[String],
        region: Option<&str>,
    ) {
        self.declaration(id, merchant);
        for country in hints {
            self.add(id, country);
        }
        if let Some(region) = region {
            self.add(id, &region.to_ascii_uppercase());
        }
    }
    pub(crate) fn outlet_country(&mut self, id: &str, country: &str) {
        self.add(id, country);
    }
    pub fn hydrate(&self, mut merchant: Merchant) -> Merchant {
        merchant.markets = self
            .countries
            .get(&merchant.id)
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        merchant
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
    use crate::location::LocationRecord;
    use crate::{import, store::MerchantStore};
    use serde_json::json;

    #[test]
    fn prepared_bundle_versions_retain_dataset_region() -> anyhow::Result<()> {
        let studio =
            r#"{"schemaVersion":"1.1.0","merchants":[{"id":"brand","canonicalName":"Brand"}]}"#;
        let mut record = import::merchant_studio(studio)?.remove(0);
        record.source = "open-enrichment".into();
        record.version = Some("fnv1a64:12345678:us:".into());
        assert_eq!(dataset_region(&record).as_deref(), Some("us"));
        assert_eq!(source_region(&record).as_deref(), Some("US"));
        record.version = Some("fnv1a64:abcdef:global:fnv1a64:12345678".into());
        assert_eq!(dataset_region(&record).as_deref(), Some("global"));
        assert!(source_region(&record).is_none());
        Ok(())
    }

    #[test]
    fn markets_combine_sources_outlets_and_manual_declarations() -> anyhow::Result<()> {
        let store = MerchantStore::temporary()?;
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
        let mut client = postgres::Client::connect(store.temporary_url(), postgres::NoTls)?;
        let stored: String = client
            .query_one("SELECT markets_json FROM merchants WHERE id=$1", &[&id])?
            .get(0);
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&stored)?,
            merchant.markets
        );
        assert!(client.query_one("SELECT to_regclass('source_market_inputs') IS NULL AND to_regclass('merchant_market_evidence') IS NULL AND NOT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='public' AND column_name='market_evidence_json')", &[])?.get::<_,bool>(0));
        assert_eq!(store.list(Some("CA"), 10, 0)?.total, 1);
        assert_eq!(store.list(Some("DE"), 10, 0)?.total, 0);
        let stats = store.stats()?;
        assert_eq!(stats.total, 1);
        assert_eq!(stats.by_market.len(), 5);
        assert_eq!(stats.by_source_region[0].region, "au");
        assert_eq!(stats.without_markets, 0);
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
        let store = MerchantStore::temporary()?;
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
        assert_eq!(store.stats()?.without_markets, 1);
        Ok(())
    }

    #[test]
    fn publisher_geography_is_not_coverage() -> anyhow::Result<()> {
        let mut records = import::catalog(
            r#"[{"id":"brand","name":"Brand","markets":["CA"]}]"#,
            "custom",
        )?;
        records[0].version = Some("us:snapshot".into());
        records[0].raw = json!({"publisherCountry":"US","countryHints":["NZ","invalid"]});
        let store = MerchantStore::temporary()?;
        store.import(&records)?;
        let merchant = store.list(None, 10, 0)?.merchants.remove(0);
        assert_eq!(merchant.markets, ["CA", "NZ"]);
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
