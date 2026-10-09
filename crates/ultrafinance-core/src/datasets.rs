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
    LunchMoney,
    BusinessTransactions,
    Foursquare,
}
impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Self::MerchantStudio => "merchant-studio",
            Self::OpenEnrichment => "open-enrichment",
            Self::DoDataThings => "dodatathings",
            Self::MoneyVis => "moneyvis",
            Self::LunchMoney => "lunchmoney",
            Self::BusinessTransactions => "business-transactions",
            Self::Foursquare => "foursquare",
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
#[derive(Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "is_zero")]
    pub skipped_transactions: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub skipped_places: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub retained_places: usize,
}
fn is_zero(value: &usize) -> bool {
    *value == 0
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

fn adapter_version(source: Source) -> u32 {
    if matches!(source, Source::LunchMoney | Source::Foursquare) {
        2
    } else {
        1
    }
}
fn bundle_path(
    directory: &Path,
    source: &str,
    version: u32,
    region: &str,
    input: &str,
    examples: Option<&str>,
) -> std::path::PathBuf {
    directory.join(source).join(format!(
        "v{}-{}-{}-{}",
        version,
        crate::eval::fingerprint(region.as_bytes()).replace("fnv1a64:", ""),
        input.replace("fnv1a64:", ""),
        examples.unwrap_or("none").replace("fnv1a64:", "")
    ))
}
/// Reuse preparation only when input, reviews, region and adapter version agree.
/// The saved knowledge file can include user-reviewed edits and stays authoritative.
pub fn cached_bundle(
    directory: &Path,
    source: Source,
    contents: &str,
    examples: Option<&str>,
    region: &str,
) -> Result<Option<(std::path::PathBuf, Manifest)>> {
    let input = crate::eval::fingerprint(contents.as_bytes());
    let examples = examples.map(|s| crate::eval::fingerprint(s.as_bytes()));
    let path = bundle_path(
        directory,
        source.name(),
        adapter_version(source),
        region,
        &input,
        examples.as_deref(),
    );
    if !path.exists() {
        return Ok(None);
    }
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(path.join("manifest.json"))?)?;
    if manifest.adapter_version != adapter_version(source)
        || manifest.source != source.name()
        || manifest.region != region
        || manifest.input_fingerprint != input
        || manifest.examples_fingerprint != examples
    {
        bail!("cached dataset metadata does not match its input");
    }
    for file in ["knowledge.json", "development.jsonl", "holdout.jsonl"] {
        if !path.join(file).is_file() {
            bail!("incomplete existing dataset at {}", path.display());
        }
    }
    Ok(Some((path, manifest)))
}

// Parse the complete JSON array, but allocate source records only for the
// selected prefix. This preserves validation and exact available counts.
pub fn read_selected_records(
    path: &Path,
    limit: Option<u32>,
) -> Result<(Vec<SourceRecord>, usize)> {
    use serde::Deserializer as _;
    struct Selected(usize);
    impl<'de> serde::de::Visitor<'de> for Selected {
        type Value = (Vec<SourceRecord>, usize);
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("an array of source records")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut records = Vec::new();
            let mut available = 0;
            while available < self.0 {
                let Some(record) = seq.next_element::<SourceRecord>()? else {
                    return Ok((records, available));
                };
                records.push(record);
                available += 1;
            }
            while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                available += 1;
            }
            Ok((records, available))
        }
    }
    let reader = std::io::BufReader::with_capacity(1024 * 1024, std::fs::File::open(path)?);
    let mut decoder = serde_json::Deserializer::from_reader(reader);
    let result = decoder.deserialize_seq(Selected(limit.map_or(usize::MAX, |n| n as usize)))?;
    decoder.end()?;
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
    let mut skipped_transactions = 0;
    let mut skipped_places = 0;
    let mut retained_places = 0;
    let (license, credit, url, kind) = match source {
        Source::Foursquare => {
            let prepared = crate::foursquare::prepare(contents, example_data, region)?;
            records = prepared.0;
            skipped_places = prepared.1;
            retained_places = records
                .iter()
                .map(|r| r.raw["places"].as_array().map_or(0, Vec::len))
                .sum();
            (
                "Apache-2.0",
                crate::foursquare::CREDIT,
                crate::foursquare::URL,
                "merchant-knowledge-only",
            )
        }
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
        Source::LunchMoney => {
            let input: Value = serde_json::from_str(contents)?;
            for row in input["transactions"]
                .as_array()
                .context("missing transactions array")?
            {
                // Group parents are edited aggregates; split children duplicate bank originals.
                let description = [
                    &row["original_name"],
                    &row["plaid_metadata"]["original_description"],
                    &row["plaid_metadata"]["name"],
                    &row["plaid_metadata"]["transaction"]["original_description"],
                    &row["plaid_metadata"]["transaction"]["name"],
                ]
                .into_iter()
                .filter_map(|value| value.as_str())
                .find(|s| !s.trim().is_empty());
                if row["plaid_account_id"].as_i64().is_none_or(|id| id <= 0)
                    || row["is_group_parent"].as_bool() == Some(true)
                    || row["split_parent_id"].as_i64().is_some_and(|id| id > 0)
                    || row["is_pending"].as_bool() == Some(true)
                    || description.is_none()
                {
                    skipped_transactions += 1;
                    continue;
                }
                samples.push(sample(
                    name,
                    description.unwrap().into(),
                    None,
                    None,
                    "real-unlabeled-merchant",
                ));
            }
            (
                "private",
                "Private Lunch Money / Plaid transaction export",
                "https://api.lunchmoney.dev/v2/transactions",
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
        Source::BusinessTransactions => {
            // A name-keyed synthetic reference catalog, not verified place identities.
            // Generated descriptions must never become aliases or inference evidence.
            let mut merchants = BTreeMap::new();
            for row in csv_rows(contents)? {
                let merchant_name = field(&row, "name")?.trim();
                let description = field(&row, "transaction_string")?.trim();
                let normalized_name = normalize(merchant_name);
                if normalized_name.is_empty() || description.is_empty() {
                    bail!("BusinessTransactions requires nonblank names and descriptions");
                }
                let id = format!(
                    "name-{}",
                    crate::eval::fingerprint(normalized_name.as_bytes())
                        .trim_start_matches("fnv1a64:")
                );
                // Choosing a deterministic spelling keeps imports independent of row order.
                merchants
                    .entry(id.clone())
                    .and_modify(|name: &mut String| {
                        if merchant_name < name.as_str() {
                            *name = merchant_name.to_owned();
                        }
                    })
                    .or_insert_with(|| merchant_name.to_owned());
                samples.push(sample(
                    name,
                    description.into(),
                    Some(expected(name, &id)),
                    row.get("category_label")
                        .filter(|s| !s.trim().is_empty())
                        .cloned(),
                    "source-synthetic-generated-merchant",
                ));
            }
            for (id, merchant_name) in merchants {
                records.push(SourceRecord {
                    source: name.into(),
                    external_id: id.clone(),
                    merchant: crate::Merchant {
                        id,
                        name: merchant_name.clone(),
                        markets: vec![],
                        market_evidence: vec![],
                        website: None,
                        logo_url: None,
                        logo_source: None,
                        aliases: vec![],
                        sources: vec!["https://huggingface.co/datasets/HighkeyPrxneeth/BusinessTransactions".into()],
                    },
                    attribution: "HighkeyPrxneeth; business names derived from Foursquare OS Places".into(),
                    license: "CC-BY-4.0 AND Apache-2.0".into(),
                    url: "https://huggingface.co/datasets/HighkeyPrxneeth/BusinessTransactions".into(),
                    version: None,
                    raw: json!({"name":merchant_name,"identity_kind":"normalized-name-only","usage":"synthetic-evaluation-reference"}),
                });
            }
            (
                "CC-BY-4.0 AND Apache-2.0",
                "HighkeyPrxneeth; business names derived from Foursquare OS Places",
                "https://huggingface.co/datasets/HighkeyPrxneeth/BusinessTransactions",
                "synthetic-source-consistency",
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
        adapter_version: adapter_version(source),
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
        skipped_transactions,
        skipped_places,
        retained_places,
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
        let path = bundle_path(
            directory,
            &self.manifest.source,
            self.manifest.adapter_version,
            &self.manifest.region,
            &self.manifest.input_fingerprint,
            self.manifest.examples_fingerprint.as_deref(),
        );
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
        if self.manifest.source == "foursquare" {
            std::fs::write(path.join("NOTICE.txt"), crate::foursquare::NOTICE)?;
            std::fs::write(path.join("LICENSE.txt"), crate::foursquare::LICENSE)?;
        }
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
    fn cached_preparation_preserves_reviewed_records_and_limited_reads() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("ultra-dataset-cache-{}", uuid::Uuid::new_v4()));
        let input = "name,transaction_string,category_label\nAlpha,ALPHA STORE,Dining\nBeta,BETA STORE,Dining\n";
        assert!(
            cached_bundle(&root, Source::BusinessTransactions, input, None, "global")?.is_none()
        );
        let bundle = prepare(Source::BusinessTransactions, input, None, "global")?;
        let path = bundle.save(&root)?;
        let mut records = bundle.records;
        records[0].merchant.name = "Reviewed Alpha".into();
        std::fs::write(path.join("knowledge.json"), serde_json::to_vec(&records)?)?;
        let (cached_path, _) =
            cached_bundle(&root, Source::BusinessTransactions, input, None, "global")?.unwrap();
        assert_eq!(cached_path, path);
        let (selected, total) = read_selected_records(&path.join("knowledge.json"), Some(1))?;
        assert_eq!(total, 2);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].merchant.name, "Reviewed Alpha");
        assert_eq!(
            read_selected_records(&path.join("knowledge.json"), None)?
                .0
                .len(),
            2
        );
        assert!(
            cached_bundle(
                &root,
                Source::BusinessTransactions,
                &format!("{input}Gamma,GAMMA,Dining\n"),
                None,
                "global"
            )?
            .is_none()
        );
        assert!(cached_bundle(&root, Source::BusinessTransactions, input, None, "ca")?.is_none());
        std::fs::write(
            path.join("knowledge.json"),
            format!("{} trailing", serde_json::to_string(&records)?),
        )?;
        assert!(read_selected_records(&path.join("knowledge.json"), Some(1)).is_err());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn business_transactions_isolates_generated_evidence_and_has_stable_name_ids() {
        let input = "name,transaction_string,category_label\nCafe Azul,CAFE AZUL STORE 0006 PHOENIX AZ $15.00,Dining\ncafe azul,CAFE AZUL PURCHASE 123,Dining\n";
        let bundle = prepare(Source::BusinessTransactions, input, None, "global").unwrap();
        assert_eq!(bundle.records.len(), 1);
        let record = &bundle.records[0];
        assert_eq!(record.merchant.name, "Cafe Azul");
        assert!(record.merchant.aliases.is_empty());
        assert!(record.merchant.markets.is_empty());
        assert!(record.merchant.website.is_none());
        assert!(record.raw.get("transaction_string").is_none());
        assert!(record.raw.get("category_label").is_none());
        assert!(
            !serde_json::to_string(&record.raw)
                .unwrap()
                .contains("PHOENIX")
        );
        assert_eq!(
            bundle.manifest.evaluation_kind,
            "synthetic-source-consistency"
        );
        for case in bundle.development.iter().chain(&bundle.holdout) {
            assert!(case.expected.is_some());
            assert_eq!(case.label_origin, "source-synthetic-generated-merchant");
            assert!(case.request.country.is_none());
            assert!(case.request.location.is_none());
            assert!(case.request.amount.is_none());
            assert!(case.request.extra.is_empty());
        }
        let reordered = "name,transaction_string,category_label\ncafe azul,CAFE AZUL PURCHASE 123,Dining\nCafe Azul,CAFE AZUL STORE 0006 PHOENIX AZ $15.00,Dining\n";
        let other = prepare(Source::BusinessTransactions, reordered, None, "global").unwrap();
        assert_eq!(other.records[0].external_id, record.external_id);
        assert_eq!(other.records[0].merchant.name, record.merchant.name);
        assert_eq!(
            serde_json::to_value(other.development).unwrap(),
            serde_json::to_value(bundle.development).unwrap()
        );
        assert_eq!(
            serde_json::to_value(other.holdout).unwrap(),
            serde_json::to_value(bundle.holdout).unwrap()
        );
    }
    #[test]
    fn business_transactions_rejects_conflicting_labels_and_blank_names() {
        let input =
            "name,transaction_string\nCafe Azul,SAME TRANSACTION\nCafe Verde,SAME TRANSACTION\n";
        assert!(
            prepare(Source::BusinessTransactions, input, None, "global")
                .err()
                .unwrap()
                .to_string()
                .contains("conflicting merchant labels")
        );
        assert!(
            prepare(
                Source::BusinessTransactions,
                "name,transaction_string\n,CAFE PURCHASE\n",
                None,
                "global"
            )
            .is_err()
        );
    }
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
    fn lunchmoney_uses_only_bank_originals_without_merchant_labels() {
        let input = json!({"transactions": [
            {"id": 1, "plaid_account_id": 2, "original_name": "BANK RAW $12", "payee": "Edited Shop", "amount": "12", "notes": "private"},
            {"id": 2, "plaid_account_id": 2, "original_name": "BANK RAW $12"},
            {"id": 3, "plaid_account_id": null, "original_name": "Manual"},
            {"id": 4, "plaid_account_id": 2, "original_name": null, "payee": "Never fallback"},
            {"id": 5, "plaid_account_id": 2, "original_name": "Split child", "split_parent_id": 1},
            {"id": 6, "plaid_account_id": 2, "original_name": "Group", "is_group_parent": true},
            {"id": 7, "plaid_account_id": 2, "original_name": "Pending", "is_pending": true},
            {"id": 8, "plaid_account_id": 2, "original_name": "   "},
            {"id": 9, "plaid_account_id": 2, "original_name": null, "payee": "Edited", "plaid_metadata": {"name": "PLAID RAW"}},
            {"id": 10, "plaid_account_id": 2, "original_name": "BANK RAW $12", "plaid_metadata": {"name": "DO NOT OVERRIDE"}}
        ]});
        let bundle = prepare(Source::LunchMoney, &input.to_string(), None, "global").unwrap();
        assert!(bundle.records.is_empty());
        assert_eq!(bundle.manifest.skipped_transactions, 6);
        let cases: Vec<_> = bundle.development.iter().chain(&bundle.holdout).collect();
        assert_eq!(cases.len(), 2);
        let descriptions: HashSet<_> = cases
            .iter()
            .map(|c| c.request.description.as_str())
            .collect();
        assert_eq!(descriptions, HashSet::from(["BANK RAW $12", "PLAID RAW"]));
        assert!(
            cases
                .iter()
                .all(|c| c.expected.is_none() && c.request.extra.is_empty())
        );
        assert!(prepare(Source::LunchMoney, "{}", None, "global").is_err());
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
