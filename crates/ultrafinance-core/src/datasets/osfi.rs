//! OSFI's public institution directory, not transaction labels or branch locations.
use crate::{Counterparty, CounterpartyType, store::SourceRecord};
use anyhow::{Context, Result, bail};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const URL: &str = "https://open.canada.ca/data/en/dataset/b27ec3ef-7338-4e76-a6fd-128339a92df5";
pub const DOWNLOAD_URL: &str = "https://open.canada.ca/data/dataset/b27ec3ef-7338-4e76-a6fd-128339a92df5/resource/945045fa-2de0-47d4-aad2-144d69467824/download/who_we_regulate_fi_eng.csv";
pub const LICENSE: &str = "Open Government Licence - Canada";
pub const CREDIT: &str = "Contains information licensed under the Open Government Licence – Canada; Office of the Superintendent of Financial Institutions (OSFI), Who we regulate";
const COLUMNS: [&str; 5] = [
    "Company Name",
    "FI Type Name",
    "FI Group Name",
    "FI Industry Name",
    "Canadian Trade Company Name",
];

fn name_key(value: &str) -> String {
    value
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn prepare(contents: &str) -> Result<Vec<SourceRecord>> {
    let mut reader = csv::Reader::from_reader(contents.trim_start_matches('\u{feff}').as_bytes());
    let headers = reader.headers()?.clone();
    let columns = COLUMNS
        .iter()
        .map(|column| {
            let matches: Vec<_> = headers
                .iter()
                .enumerate()
                .filter(|(_, h)| h == column)
                .collect();
            if matches.len() != 1 {
                bail!("OSFI CSV requires exactly one column {column}");
            }
            Ok(matches[0].0)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut records = BTreeMap::<String, SourceRecord>::new();
    for (index, row) in reader.records().enumerate() {
        let row = row.with_context(|| format!("invalid OSFI CSV row {}", index + 2))?;
        let fields: Vec<_> = columns.iter().map(|&column| row[column].trim()).collect();
        let [name, fi_type, group, industry, trade] = fields.as_slice() else {
            unreachable!()
        };
        if fields[..4].iter().any(|v| v.is_empty()) {
            bail!(
                "OSFI CSV row {} has a blank name or institution classification",
                index + 2
            );
        }
        if !matches!(
            *fi_type,
            "Federally Regulated Financial Institutions" | "Foreign Bank Representative Offices"
        ) {
            bail!(
                "unsupported OSFI institution type at row {}: {fi_type}",
                index + 2
            );
        }
        // No upstream institution ID is supplied. Keep different regulatory
        // categories distinct and ignore contacts/address/order when deriving IDs.
        let key = json!([
            name_key(name),
            name_key(fi_type),
            name_key(group),
            name_key(industry)
        ])
        .to_string();
        let digest: String = Sha256::digest(key.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let external_id = format!("name:{digest}");
        let record = records
            .entry(external_id.clone())
            .or_insert_with(|| SourceRecord {
                source: "osfi".into(),
                external_id: external_id.clone(),
                merchant: Counterparty {
                    id: format!("osfi:{external_id}"),
                    name: (*name).into(),
                    counterparty_type: CounterpartyType::FinancialInstitution,
                    markets: vec!["CA".into()],
                    aliases: vec![],
                    website: None,
                    logo_url: None,
                    logo_source: None,
                    sources: vec![URL.into()],
                },
                attribution: CREDIT.into(),
                license: LICENSE.into(),
                url: URL.into(),
                version: None,
                raw: json!({"identity_kind":"normalized-name-and-regulatory-classification",
                "company_name":name,"fi_type":fi_type,"fi_group":group,"fi_industry":industry}),
            });
        // Duplicate contact rows are one source identity. Keep only official
        // trade names as aliases; never generate statement descriptors.
        if !trade.is_empty() && name_key(trade) != name_key(name) {
            record.merchant.aliases.push((*trade).into());
        }
    }
    if records.is_empty() {
        bail!("OSFI CSV contains no financial institutions");
    }
    for record in records.values_mut() {
        record.merchant.aliases.sort();
        let mut seen = BTreeSet::new();
        record
            .merchant
            .aliases
            .retain(|alias| seen.insert(name_key(alias)));
        record.raw["trade_names"] = json!(record.merchant.aliases);
    }
    Ok(records.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    const FIXTURE: &str = include_str!("../../tests/fixtures/osfi.csv");

    #[test]
    fn osfi_imports_typed_identities_without_contacts_or_invented_evidence() -> Result<()> {
        let records = prepare(&format!("\u{feff}{FIXTURE}"))?;
        assert_eq!(records.len(), 3);
        let bank = records
            .iter()
            .find(|r| r.raw["fi_group"] == "Banks")
            .unwrap();
        assert_eq!(bank.merchant.aliases, ["Example Banking", "Example Banque"]);
        for record in &records {
            assert_eq!(
                record.merchant.counterparty_type,
                CounterpartyType::FinancialInstitution
            );
            assert_eq!(record.merchant.markets, ["CA"]);
            assert!(record.merchant.website.is_none() && record.merchant.logo_url.is_none());
            assert_eq!(record.license, LICENSE);
            assert!(record.external_id.starts_with("name:"));
        }
        let serialized = serde_json::to_string(&records)?;
        assert!(!serialized.contains("Test Person"));
        assert!(!serialized.contains("Main St"));
        assert!(!serialized.contains("transaction_pattern"));
        let changed = prepare(
            &FIXTURE
                .replace("1 Main St", "99 Changed St")
                .replace("Test Person", "New Person"),
        )?;
        assert_eq!(serialized, serde_json::to_string(&changed)?);
        let mut lines = FIXTURE.lines();
        let header = lines.next().unwrap();
        let mut rows: Vec<_> = lines.collect();
        rows.reverse();
        let reversed = prepare(&format!("{header}\n{}\n", rows.join("\n")))?;
        assert_eq!(serialized, serde_json::to_string(&reversed)?);
        Ok(())
    }

    #[test]
    fn osfi_rejects_schema_drift_invalid_rows_and_transaction_examples() {
        for invalid in [
            "Company Name\nExample\n".to_owned(),
            FIXTURE.lines().next().unwrap().to_owned(),
            FIXTURE.replace("Domestic Banks", ""),
            FIXTURE.replace(
                "Federally Regulated Financial Institutions",
                "Pension Plans",
            ),
            FIXTURE.replacen("Company Name", "Company Name,Company Name", 1),
        ] {
            assert!(prepare(&invalid).is_err(), "accepted malformed directory");
        }
        use crate::datasets::{Source, prepare as bundle};
        assert!(bundle(Source::Osfi, FIXTURE, Some("examples"), "ca").is_err());
        assert!(bundle(Source::Osfi, FIXTURE, None, "us").is_err());
        let prepared = bundle(Source::Osfi, FIXTURE, None, "ca").unwrap();
        assert!(prepared.holdout.is_empty() && prepared.development.is_empty());
        assert_eq!(
            prepared.manifest.evaluation_kind,
            "counterparty-knowledge-only"
        );
        assert!(prepared.records.iter().all(|r| r.version.is_some()));
    }
}
