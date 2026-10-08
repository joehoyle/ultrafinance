use crate::{
    EnrichRequest, import,
    store::{SourceRecord, normalize},
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

#[derive(Clone, Copy)]
pub enum Source {
    MerchantStudio,
    OpenEnrichment,
    DoDataThings,
    MoneyVis,
}
impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Self::MerchantStudio => "merchant-studio",
            Self::OpenEnrichment => "open-enrichment",
            Self::DoDataThings => "dodatathings",
            Self::MoneyVis => "moneyvis",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub id: String,
    pub request: EnrichRequest,
    pub expected: Option<Value>,
    pub category: Option<String>,
    pub label_origin: String,
    pub source: String,
}
#[derive(Serialize)]
pub struct Manifest {
    pub adapter_version: u32,
    pub source: String,
    pub region: String,
    pub input_fingerprint: String,
    pub examples_fingerprint: Option<String>,
    pub license: String,
    pub attribution: String,
    pub source_url: String,
    pub evaluation_kind: String,
    pub split_policy: String,
    pub development_samples: usize,
    pub holdout_samples: usize,
    pub labeled_development: usize,
    pub labeled_holdout: usize,
    pub merchant_records: usize,
    pub skipped_child_records: usize,
}
pub struct Bundle {
    pub records: Vec<SourceRecord>,
    pub development: Vec<Sample>,
    pub holdout: Vec<Sample>,
    pub manifest: Manifest,
}

fn request(description: String) -> EnrichRequest {
    EnrichRequest {
        description,
        amount: None,
        currency: None,
        date: None,
        country: None,
        location: None,
        extra: Default::default(),
    }
}
fn expected(source: &str, id: &str) -> Value {
    json!({"status":"matched","merchant":{"source":source,"external_id":id}})
}
fn sample(
    source: &str,
    description: String,
    label: Option<Value>,
    category: Option<String>,
    origin: &str,
) -> Sample {
    let id = format!(
        "{source}-{}",
        crate::eval::fingerprint(normalize(&description).as_bytes()).replace("fnv1a64:", "")
    );
    Sample {
        id,
        request: request(description),
        expected: label,
        category,
        label_origin: origin.into(),
        source: source.into(),
    }
}
fn csv_rows(contents: &str) -> Result<Vec<BTreeMap<String, String>>> {
    let mut reader = csv::Reader::from_reader(contents.trim_start_matches('\u{feff}').as_bytes());
    let headers = reader.headers()?.clone();
    let mut rows = Vec::new();
    for row in reader.records() {
        let row = row?;
        rows.push(
            headers
                .iter()
                .zip(row.iter())
                .map(|(a, b)| (a.into(), b.into()))
                .collect(),
        );
    }
    Ok(rows)
}
fn field<'a>(row: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str> {
    row.get(name)
        .map(String::as_str)
        .with_context(|| format!("missing CSV column {name}"))
}
fn examples(value: &str) -> Result<Vec<String>> {
    // Open Enrichment uses [`text`, `text`] rather than JSON strings.
    if value.trim().is_empty() {
        return Ok(vec![]);
    }
    let inner = value
        .trim()
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .context("invalid transaction_text_examples list")?;
    let mut result = Vec::new();
    let mut remaining = inner.trim();
    while !remaining.is_empty() {
        let rest = remaining
            .strip_prefix('`')
            .context("expected backtick-delimited transaction example")?;
        let end = rest.find('`').context("unterminated transaction example")?;
        result.push(rest[..end].to_string());
        remaining = rest[end + 1..].trim();
        if !remaining.is_empty() {
            remaining = remaining
                .strip_prefix(',')
                .context("expected comma between examples")?
                .trim();
        }
    }
    Ok(result)
}

pub fn prepare(
    source: Source,
    contents: &str,
    example_data: Option<&str>,
    region: &str,
) -> Result<Bundle> {
    let name = source.name();
    let snapshot = crate::eval::fingerprint(contents.as_bytes());
    let mut samples = Vec::new();
    let mut records = Vec::new();
    let mut skipped = 0;
    let (license, credit, url, kind) = match source {
        Source::MerchantStudio => {
            records = import::merchant_studio(contents)?;
            let input: Value = serde_json::from_str(
                example_data.context("Merchant Studio requires --examples FILE")?,
            )?;
            if !input["schemaVersion"]
                .as_str()
                .is_some_and(|s| s.starts_with("1."))
            {
                bail!("unsupported Merchant Studio examples schema");
            }
            for row in input["descriptors"]
                .as_array()
                .context("missing descriptors array")?
            {
                let description = row["rawDescription"]
                    .as_str()
                    .context("missing rawDescription")?
                    .to_string();
                let label = row["expected"]["merchantId"]
                    .as_str()
                    .map(|id| expected(name, id));
                let mut case = sample(
                    name,
                    description,
                    label,
                    row["expected"]["category"].as_str().map(String::from),
                    "source-synthetic",
                );
                case.request.country = match row["region"].as_str() {
                    Some("US" | "CA" | "FL") => Some("US".into()),
                    Some("DO" | "MX" | "BR" | "ES") => row["region"].as_str().map(String::from),
                    _ => None,
                };
                samples.push(case);
            }
            (
                "CC-BY-4.0",
                import::STUDIO_CREDIT,
                import::STUDIO_URL,
                "source-consistency",
            )
        }
        Source::OpenEnrichment => {
            let rows = csv_rows(contents)?;
            for row in rows {
                let id = field(&row, "id")?;
                let brand = field(&row, "name")?;
                let descriptions = examples(field(&row, "transaction_text_examples")?)?;
                let parent = field(&row, "parent_id")?;
                for description in &descriptions {
                    // Child place identities need a location adapter; retain for manual labeling.
                    let label = parent.is_empty().then(|| expected(name, id));
                    samples.push(sample(
                        name,
                        description.clone(),
                        label,
                        None,
                        "source-reviewed",
                    ));
                }
                if !parent.is_empty() {
                    skipped += 1;
                    continue;
                }
                let merchant = crate::Merchant {
                    id: id.into(),
                    name: brand.into(),
                    markets: vec![],
                    market_evidence: vec![],
                    website: (!field(&row, "website_url")?.is_empty())
                        .then(|| row["website_url"].clone()),
                    logo_url: None,
                    logo_source: None,
                    aliases: descriptions,
                    sources: vec!["https://github.com/steveharrison/openenrichment".into()],
                };
                records.push(SourceRecord {
                    source: name.into(),
                    external_id: id.into(),
                    merchant,
                    attribution: "Open Enrichment by Steve Harrison".into(),
                    license: "CC0-1.0".into(),
                    url: "https://github.com/steveharrison/openenrichment".into(),
                    version: Some(format!("{region}:{snapshot}")),
                    raw: json!(row),
                });
            }
            (
                "CC0-1.0",
                "Open Enrichment by Steve Harrison",
                "https://github.com/steveharrison/openenrichment",
                "source-consistency",
            )
        }
        Source::DoDataThings => {
            for row in csv_rows(contents)? {
                let mut case = sample(
                    name,
                    field(&row, "description")?.into(),
                    None,
                    Some(field(&row, "category")?.into()),
                    "source-synthetic-category-only",
                );
                case.request.country = Some("US".into());
                samples.push(case);
            }
            (
                "MIT",
                "DoDataThings us-bank-transaction-categories-v2",
                "https://huggingface.co/datasets/DoDataThings/us-bank-transaction-categories-v2",
                "requires-merchant-labels",
            )
        }
        Source::MoneyVis => {
            for row in csv_rows(contents)? {
                let mut case = sample(
                    name,
                    field(&row, "Transaction Description")?.into(),
                    None,
                    None,
                    "real-unlabeled-merchant",
                );
                // Intentionally discard account, sort code and balance columns.
                let debit = field(&row, "Debit Amount")?;
                let credit = field(&row, "Credit Amount")?;
                case.request.amount = if !debit.trim().is_empty() {
                    Some(debit.into())
                } else if !credit.trim().is_empty() {
                    Some(format!("-{credit}"))
                } else {
                    None
                };
                case.request.extra.insert(
                    "transaction_type".into(),
                    json!(field(&row, "Transaction Type")?),
                );
                samples.push(case);
            }
            (
                "CC-BY-4.0",
                "MoneyData / MoneyVis by Robert Laramee and collaborators",
                "https://data.mendeley.com/datasets/dnxtg6n4rv/1",
                "requires-merchant-labels",
            )
        }
    };
    // Deduplicate exact normalized descriptions, rejecting conflicting labels.
    let mut unique: BTreeMap<String, Sample> = BTreeMap::new();
    for case in samples {
        case.request.validate()?;
        let key = normalize(&case.request.description);
        if let Some(existing) = unique.get_mut(&key) {
            if existing.expected.is_some()
                && case.expected.is_some()
                && existing.expected != case.expected
            {
                bail!("conflicting merchant labels for normalized description {key}");
            }
            if existing.expected.is_none() {
                existing.expected = case.expected;
            }
            if existing.category != case.category {
                existing.category = None;
            }
        } else {
            unique.insert(key, case);
        }
    }
    let mut development = Vec::new();
    let mut holdout = Vec::new();
    for (key, case) in unique {
        let hash = crate::eval::fingerprint(format!("ultrafinance-split-v1:{key}").as_bytes());
        let value = u64::from_str_radix(hash.trim_start_matches("fnv1a64:"), 16)?;
        if value % 5 == 0 {
            holdout.push(case);
        } else {
            development.push(case);
        }
    }
    let held: HashSet<_> = holdout
        .iter()
        .map(|c| normalize(&c.request.description))
        .collect();
    for record in &mut records {
        record
            .merchant
            .aliases
            .retain(|a| !held.contains(&normalize(a)));
        // Keep held-out examples in snapshots, not inference evidence or alias indexes.
        if let Some(object) = record.raw.as_object_mut() {
            object.remove("transaction_text_examples");
            if object.contains_key("aliases") {
                object.insert("aliases".into(), json!(record.merchant.aliases));
            }
        }
        record.version = Some(format!(
            "{}:{}:{}",
            snapshot,
            region,
            example_data
                .map(|s| crate::eval::fingerprint(s.as_bytes()))
                .unwrap_or_default()
        ));
    }
    let manifest = Manifest {
        adapter_version: 1,
        source: name.into(),
        region: region.into(),
        input_fingerprint: snapshot,
        examples_fingerprint: example_data.map(|s| crate::eval::fingerprint(s.as_bytes())),
        license: license.into(),
        attribution: credit.into(),
        source_url: url.into(),
        evaluation_kind: kind.into(),
        split_policy:
            "normalized description; fixed split-v1 hash; ~80% development / ~20% holdout".into(),
        development_samples: development.len(),
        holdout_samples: holdout.len(),
        labeled_development: development.iter().filter(|s| s.expected.is_some()).count(),
        labeled_holdout: holdout.iter().filter(|s| s.expected.is_some()).count(),
        merchant_records: records.len(),
        skipped_child_records: skipped,
    };
    Ok(Bundle {
        records,
        development,
        holdout,
        manifest,
    })
}

fn write_samples(path: &Path, samples: &[Sample]) -> Result<()> {
    use std::io::Write;
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    for sample in samples {
        writeln!(file, "{}", serde_json::to_string(sample)?)?;
    }
    file.flush()?;
    Ok(())
}
pub fn eval_suite(name: &str, samples: &[Sample]) -> Value {
    let cases: Vec<_> = samples
        .iter()
        .filter_map(|s| {
            s.expected
                .as_ref()
                .map(|label| json!({"id":s.id,"request":s.request,"expected":label}))
        })
        .collect();
    json!({"version":1,"name":name,"cases":cases})
}
impl Bundle {
    /// Output is content-addressed; existing prepared versions are never overwritten.
    pub fn save(&self, directory: &Path) -> Result<std::path::PathBuf> {
        let version = format!(
            "v{}-{}-{}-{}",
            self.manifest.adapter_version,
            crate::eval::fingerprint(self.manifest.region.as_bytes()).replace("fnv1a64:", ""),
            self.manifest.input_fingerprint.replace("fnv1a64:", ""),
            self.manifest
                .examples_fingerprint
                .as_deref()
                .unwrap_or("none")
                .replace("fnv1a64:", "")
        );
        let path = directory.join(&self.manifest.source).join(version);
        if path.exists() {
            let existing: Value =
                serde_json::from_str(&std::fs::read_to_string(path.join("manifest.json"))?)?;
            if existing != serde_json::to_value(&self.manifest)? {
                bail!("existing dataset manifest differs at {}", path.display());
            }
            for file in ["knowledge.json", "development.jsonl", "holdout.jsonl"] {
                if !path.join(file).is_file() {
                    bail!("incomplete existing dataset at {}", path.display());
                }
            }
            return Ok(path);
        }
        let final_path = path;
        let path = directory
            .join(&self.manifest.source)
            .join(format!(".preparing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path)?;
        write_samples(&path.join("development.jsonl"), &self.development)?;
        write_samples(&path.join("holdout.jsonl"), &self.holdout)?;
        for (split, samples) in [
            ("development", &self.development),
            ("holdout", &self.holdout),
        ] {
            if samples.iter().any(|s| s.expected.is_some()) {
                std::fs::write(
                    path.join(format!("{split}.eval.json")),
                    serde_json::to_string_pretty(&eval_suite(
                        &format!("{}-{split}", self.manifest.source),
                        samples,
                    ))?,
                )?;
            }
        }
        std::fs::write(
            path.join("knowledge.json"),
            serde_json::to_string_pretty(&self.records)?,
        )?;
        std::fs::write(
            path.join("manifest.json"),
            serde_json::to_string_pretty(&self.manifest)?,
        )?;
        std::fs::rename(&path, &final_path)?;
        Ok(final_path)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn category_labels_do_not_turn_into_merchant_labels_and_splits_are_stable() {
        let input = "description,category\nADIDAS,Shopping\nPAYMENT FROM ADIDAS,Shopping\nNETFLIX,Subscription\n";
        let a = prepare(Source::DoDataThings, input, None, "global").unwrap();
        let b = prepare(Source::DoDataThings, "description,category\nNETFLIX,Subscription\nPAYMENT FROM ADIDAS,Shopping\nADIDAS,Shopping\n", None, "global").unwrap();
        assert_eq!(
            serde_json::to_value(&a.holdout).unwrap(),
            serde_json::to_value(&b.holdout).unwrap()
        );
        assert_eq!(
            a.manifest.labeled_development + a.manifest.labeled_holdout,
            0
        );
        assert!(a.records.is_empty());
    }
    #[test]
    fn moneyvis_discards_account_details() {
        let input = "Transaction Description,Transaction Type,Debit Amount,Credit Amount,Account Number,Sort Code,Balance\nLOCAL CAFE,DEB,4.50,,12345678,12-34-56,1000\n";
        let bundle = prepare(Source::MoneyVis, input, None, "global").unwrap();
        let sample = bundle
            .development
            .iter()
            .chain(&bundle.holdout)
            .next()
            .unwrap();
        assert_eq!(sample.request.amount.as_deref(), Some("4.50"));
        let serialized = serde_json::to_string(sample).unwrap();
        for sensitive in ["12345678", "12-34-56", "1000"] {
            assert!(!serialized.contains(sensitive));
        }
        assert!(sample.expected.is_none());
    }

    #[test]
    fn rerun_preserves_user_labels_and_snapshot_path() {
        let bundle = prepare(
            Source::DoDataThings,
            "description,category\nADIDAS,Shopping\n",
            None,
            "global",
        )
        .unwrap();
        let directory =
            std::env::temp_dir().join(format!("ultra-datasets-{}", uuid::Uuid::new_v4()));
        let path = bundle.save(&directory).unwrap();
        std::fs::write(path.join("development.jsonl"), "user-labeled data").unwrap();
        assert_eq!(bundle.save(&directory).unwrap(), path);
        assert_eq!(
            std::fs::read_to_string(path.join("development.jsonl")).unwrap(),
            "user-labeled data"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn open_enrichment_keeps_regex_as_evidence_and_excludes_holdout_aliases() {
        let input = "id,name,parent_id,website_url,transaction_text_examples,transaction_text_regexp\na,Adidas,,https://adidas.com,\"[`ADIDAS`, `PAYMENT FROM ADIDAS`]\",(?i)ADIDAS\n";
        let bundle = prepare(Source::OpenEnrichment, input, None, "global").unwrap();
        assert_eq!(bundle.records.len(), 1);
        assert_eq!(
            bundle.records[0].raw["transaction_text_regexp"],
            "(?i)ADIDAS"
        );
        assert!(
            bundle.records[0]
                .raw
                .get("transaction_text_examples")
                .is_none()
        );
        for sample in bundle.holdout {
            assert!(
                !bundle.records[0]
                    .merchant
                    .aliases
                    .iter()
                    .any(|a| normalize(a) == normalize(&sample.request.description))
            );
        }
        assert!(bundle.records[0].merchant.logo_url.is_none());
    }
}
