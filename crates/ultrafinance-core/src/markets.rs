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
#[path = "../tests/unit/markets/tests.rs"]
mod tests;

#[cfg(test)]
include!("../tests/unit/markets/support.rs");
