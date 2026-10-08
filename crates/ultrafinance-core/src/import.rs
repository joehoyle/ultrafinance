use crate::{Merchant, store::SourceRecord};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;

pub const STUDIO_URL: &str = "https://github.com/jtvargas/merchant-studio";
pub const STUDIO_CREDIT: &str = "Enrichment from Merchant Studio by Jonathan Taveras";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StudioDataset {
    schema_version: String,
    generated_at: Option<String>,
    merchants: Vec<Value>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StudioMerchant {
    id: String,
    canonical_name: String,
    #[serde(default)]
    aliases: Vec<String>,
    website: Option<String>,
}

/// Country hints, categories, negative aliases and other source fields remain in raw evidence.
/// Country hints are not definitive merchant operating-country restrictions.
pub fn merchant_studio(contents: &str) -> Result<Vec<SourceRecord>> {
    let dataset: StudioDataset =
        serde_json::from_str(contents).context("invalid Merchant Studio dataset")?;
    if !dataset.schema_version.starts_with("1.") {
        bail!("unsupported Merchant Studio schema version");
    }
    dataset
        .merchants
        .into_iter()
        .map(|raw| {
            let entry: StudioMerchant =
                serde_json::from_value(raw.clone()).context("invalid Merchant Studio merchant")?;
            let merchant = Merchant {
                id: entry.id.clone(),
                name: entry.canonical_name,
                country: None,
                website: entry.website.map(|s| {
                    if s.starts_with("https://") || s.starts_with("http://") {
                        s
                    } else {
                        format!("https://{s}")
                    }
                }),
                logo_url: None,
                logo_source: None,
                aliases: entry.aliases,
                sources: vec![STUDIO_URL.into()],
            };
            Ok(SourceRecord {
                source: "merchant-studio".into(),
                external_id: entry.id,
                merchant,
                attribution: STUDIO_CREDIT.into(),
                license: "CC-BY-4.0".into(),
                url: STUDIO_URL.into(),
                version: Some(format!(
                    "{}:{}",
                    dataset.schema_version,
                    dataset.generated_at.as_deref().unwrap_or("unknown")
                )),
                raw,
            })
        })
        .collect()
}

pub fn catalog(contents: &str, source: &str) -> Result<Vec<SourceRecord>> {
    let merchants: Vec<Merchant> =
        serde_json::from_str(contents).context("invalid merchant catalog")?;
    merchants
        .into_iter()
        .map(|mut merchant| {
            if merchant.logo_url.is_some() && merchant.logo_source.is_none() {
                merchant.logo_source = Some(source.into());
            }
            Ok(SourceRecord {
                source: source.into(),
                external_id: merchant.id.clone(),
                raw: serde_json::to_value(&merchant)?,
                merchant,
                attribution: source.into(),
                license: "unspecified".into(),
                url: String::new(),
                version: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn studio_keeps_provenance_and_country_hints_without_claiming_identity() {
        let input = r#"{"schemaVersion":"1.1.0","generatedAt":"2026-07-06","merchants":[{"id":"amazon","canonicalName":"Amazon","aliases":["amzn mktp"],"website":"amazon.com","countryHints":["US","CA"],"negativeAliases":["aws"]}]}"#;
        let records = merchant_studio(input).unwrap();
        assert_eq!(records[0].source, "merchant-studio");
        assert_eq!(records[0].external_id, "amazon");
        assert_eq!(records[0].merchant.country, None);
        assert_eq!(
            records[0].merchant.website.as_deref(),
            Some("https://amazon.com")
        );
        assert_eq!(records[0].raw["countryHints"][1], "CA");
        assert_eq!(records[0].license, "CC-BY-4.0");
        assert!(merchant_studio(r#"{"schemaVersion":"2.0","merchants":[]}"#).is_err());
    }
}
