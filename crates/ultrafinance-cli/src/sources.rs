//! Registered upstream sources and immutable local downloads.
use super::{DatasetSource, EvalMode, batch};
use anyhow::{Context, Result, bail};
use clap::{Args as ClapArgs, Subcommand};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use ultrafinance_core::{
    datasets::{self, Source},
    store::MerchantStore,
};

#[derive(ClapArgs)]
pub struct Args {
    /// Show detailed per-batch import diagnostics.
    #[arg(long, global = true)]
    verbose: bool,
    /// Directory holding original downloads and snapshot metadata.
    #[arg(long, global = true, default_value = "data/sources")]
    cache_dir: PathBuf,
    /// Lunch Money executable (uses its saved authentication or LUNCH_MONEY_TOKEN).
    #[arg(
        long,
        global = true,
        env = "ULTRAFINANCE_LUNCHMONEY_CLI",
        default_value = "lunchmoney"
    )]
    lunchmoney_cli: PathBuf,
    /// DuckDB executable for authenticated Foursquare Iceberg downloads.
    #[arg(
        long,
        global = true,
        env = "ULTRAFINANCE_DUCKDB_CLI",
        default_value = "duckdb"
    )]
    duckdb_cli: PathBuf,
    /// Maximum Foursquare rows to download; omitted means all open places in the selected region.
    #[arg(long, global = true, value_parser = clap::value_parser!(u32).range(1..))]
    foursquare_limit: Option<u32>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Evaluate a source's holdout against the configured catalog, without importing knowledge.
    Eval {
        #[arg(value_enum)]
        source: DatasetSource,
        #[arg(long, default_value = "global")]
        region: String,
        /// Fetch a fresh snapshot before evaluation; otherwise reuse the cached download.
        #[arg(long, conflicts_with = "offline")]
        refresh: bool,
        /// Require a cached download and never contact the upstream.
        #[arg(long)]
        offline: bool,
        #[arg(long, default_value = "data/datasets")]
        datasets_dir: PathBuf,
        #[arg(long, value_enum, default_value = "search")]
        mode: EvalMode,
        /// Show each input record, ranked candidates and final match result.
        #[arg(long)]
        details: bool,
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
        limit: Option<u32>,
        /// Report path; defaults to evals/reports/SOURCE-REGION-holdout-MODE.json.
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long, env = "JEV_MODEL", default_value = "jev-latest")]
        model: String,
        #[arg(long, env = "ULTRAFINANCE_MATCH_THRESHOLD", default_value = "0.95")]
        threshold: f64,
    },
    /// List the built-in source registry (no database or network required).
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show source purpose, upstream URLs and latest local download.
    Show {
        #[arg(value_enum)]
        source: DatasetSource,
        #[arg(long, default_value = "global")]
        region: String,
    },
    /// Fetch original upstream files into an immutable local snapshot.
    Download {
        #[arg(value_enum)]
        source: DatasetSource,
        #[arg(long, default_value = "global")]
        region: String,
    },
    /// Download, prepare and apply only added or changed source records.
    Import {
        #[arg(value_enum)]
        source: DatasetSource,
        #[arg(long, default_value = "global")]
        region: String,
        /// Use the latest local download without contacting the upstream.
        #[arg(long, conflicts_with = "input")]
        offline: bool,
        /// Use a manually downloaded file instead of contacting the upstream.
        #[arg(long)]
        input: Option<PathBuf>,
        /// Merchant Studio descriptors or Foursquare reviewed brand mappings.
        #[arg(long)]
        examples: Option<PathBuf>,
        #[arg(long, default_value = "data/datasets")]
        output: PathBuf,
        /// Prepare a bundle and report its size without changing the database.
        #[arg(long)]
        dry_run: bool,
        /// Allow the names-only synthetic BusinessTransactions evaluation catalog.
        #[arg(long)]
        evaluation: bool,
        #[command(flatten)]
        dedupe: super::ImportDedupeArgs,
    },
    /// Browse original downloaded rows, including data excluded from knowledge imports.
    Raw {
        #[arg(value_enum)]
        source: DatasetSource,
        #[arg(long, default_value = "global")]
        region: String,
        #[arg(long, default_value = "20", value_parser = clap::value_parser!(u32).range(1..=1000))]
        limit: u32,
        #[arg(long, default_value = "0")]
        offset: usize,
        /// Inspect Merchant Studio's example descriptors instead of merchants.
        #[arg(long)]
        examples: bool,
    },
    /// Browse imported source payloads and their local merchant mappings.
    Records {
        /// Source namespace; also accepts namespaces from custom catalog imports.
        source: String,
        #[arg(long)]
        external_id: Option<String>,
        #[arg(long, default_value = "20", value_parser = clap::value_parser!(u32).range(1..=1000))]
        limit: u32,
        #[arg(long, default_value = "0")]
        offset: usize,
    },
}

struct Registration {
    source: Source,
    purpose: &'static str,
    url: &'static str,
    license: &'static str,
}
fn registry(source: DatasetSource) -> Registration {
    let (source, purpose, url, license) = match source {
        DatasetSource::Foursquare => (
            Source::Foursquare,
            "Merchant and outlet knowledge; optional reviewed brand grouping",
            "https://docs.foursquare.com/data-products/docs/access-fsq-os-places",
            "Apache-2.0",
        ),
        DatasetSource::MerchantStudio => (
            Source::MerchantStudio,
            "Merchant names and aliases",
            "https://github.com/jtvargas/merchant-studio",
            "CC-BY-4.0",
        ),
        DatasetSource::OpenEnrichment => (
            Source::OpenEnrichment,
            "Merchant names and transaction patterns",
            "https://github.com/steveharrison/openenrichment",
            "CC0-1.0",
        ),
        DatasetSource::Dodatathings => (
            Source::DoDataThings,
            "Synthetic category evaluation; no merchant knowledge",
            "https://huggingface.co/datasets/DoDataThings/us-bank-transaction-categories-v2",
            "MIT",
        ),
        DatasetSource::Lunchmoney => (
            Source::LunchMoney,
            "Private real Plaid transaction evaluation; no merchant knowledge",
            "https://api.lunchmoney.dev/v2/transactions",
            "private",
        ),
        DatasetSource::Moneyvis => (
            Source::MoneyVis,
            "Real anonymized transaction evaluation; no merchant knowledge",
            "https://data.mendeley.com/datasets/dnxtg6n4rv/1",
            "CC-BY-4.0",
        ),
        DatasetSource::BusinessTransactions => (
            Source::BusinessTransactions,
            "Synthetic evaluation; names-only reference catalog",
            "https://huggingface.co/datasets/HighkeyPrxneeth/BusinessTransactions",
            "CC-BY-4.0 AND Apache-2.0",
        ),
    };
    Registration {
        source,
        purpose,
        url,
        license,
    }
}
fn urls(source: Source, region: &str) -> Result<Vec<(&'static str, String)>> {
    if matches!(source, Source::OpenEnrichment | Source::Foursquare) {
        if region != "global"
            && !(region.len() == 2 && region.bytes().all(|b| b.is_ascii_lowercase()))
        {
            bail!(
                "Open Enrichment region must be global or a lowercase two-letter code (e.g. us, uk, au)"
            );
        }
    } else if region != "global" {
        bail!("this source supports only --region global");
    }
    Ok(match source {
        Source::Foursquare => vec![],
        Source::MerchantStudio => vec![
            ("input.json", "https://jtvargas.github.io/merchant-studio/data/merchant_aliases.json".into()),
            ("examples.json", "https://jtvargas.github.io/merchant-studio/data/sample_test_descriptors.json".into()),
        ],
        Source::OpenEnrichment => vec![("input.csv", format!("https://raw.githubusercontent.com/steveharrison/openenrichment/main/src/public/data/{region}/merchants.csv"))],
        Source::DoDataThings => vec![("input.csv", "https://huggingface.co/datasets/DoDataThings/us-bank-transaction-categories-v2/resolve/main/transactions-synthetic.csv".into())],
        Source::BusinessTransactions => vec![("input.csv", "https://huggingface.co/datasets/HighkeyPrxneeth/BusinessTransactions/resolve/main/business_transactions_dataset.csv".into())],
        Source::LunchMoney => vec![("input.json", "cli://lunchmoney/transactions?period=last-year".into())],
        Source::MoneyVis => vec![("input.csv", "https://raw.githubusercontent.com/thevisgroup/MoneyVis/master/data/data.csv".into())],
    })
}
fn metadata(r: &Registration) -> Value {
    json!({"source": r.source.name(), "purpose": r.purpose, "url": r.url, "license": r.license,
        "download": if matches!(r.source, Source::Foursquare) { "automatic-authenticated" } else { "automatic" }, "transport": if matches!(r.source, Source::Foursquare) { "duckdb-iceberg" } else if matches!(r.source, Source::LunchMoney) { "lunchmoney-cli" } else { "http" }})
}
fn cache_path(root: &Path, source: Source, region: &str) -> Result<PathBuf> {
    urls(source, region)?;
    Ok(root.join(source.name()).join(region))
}
fn latest(root: &Path, source: Source, region: &str) -> Result<PathBuf> {
    let parent = cache_path(root, source, region)?;
    let name = std::fs::read_to_string(parent.join("latest"))
        .context("no local snapshot; run sources download first (or import with --input FILE)")?;
    if !name.starts_with("snapshot-")
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        bail!("invalid local snapshot pointer");
    }
    Ok(parent.join(name))
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp =
        tempfile::NamedTempFile::new_in(path.parent().context("missing parent directory")?)?;
    tmp.write_all(bytes)?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}
async fn fetch(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    const MAX_BYTES: usize = 64 * 1024 * 1024;
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        bail!("download exceeds 64 MiB");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > MAX_BYTES {
            bail!("download exceeds 64 MiB");
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        bail!("upstream returned an empty file");
    }
    std::str::from_utf8(&bytes).context("source download is not UTF-8")?;
    Ok(bytes)
}
async fn download(
    root: &Path,
    r: &Registration,
    region: &str,
    lunchmoney_cli: &Path,
    duckdb_cli: &Path,
    foursquare_limit: Option<u32>,
) -> Result<PathBuf> {
    urls(r.source, region)?;
    if matches!(r.source, Source::Foursquare) {
        let token = std::env::var("ULTRAFINANCE_FOURSQUARE_TOKEN")
            .ok().filter(|s| !s.trim().is_empty())
            .context("Set ULTRAFINANCE_FOURSQUARE_TOKEN to an authenticated Places Portal access token (not a Places API key)")?;
        let executable = duckdb_cli.to_owned();
        let country = region.to_owned();
        let limit_label = foursquare_limit
            .map(|n| n.to_string())
            .unwrap_or_else(|| "all".into());
        eprintln!("Downloading Foursquare: region={region}, maximum rows={limit_label}");
        let export = tokio::task::spawn_blocking(move || {
            super::foursquare_download::export(&executable, &token, &country, foursquare_limit)
        })
        .await??;
        let url = format!(
            "https://catalog.h3-hub.foursquare.com/iceberg?warehouse=places&table=datasets.places_os&region={region}&limit={limit_label}"
        );
        return save_foursquare_snapshot(root,r,region,&export.path().join("places.csv"),url);
    }
    let files = urls(r.source, region)?;
    if matches!(r.source, Source::LunchMoney) {
        let (start, end) = last_year(std::time::SystemTime::now());
        eprintln!("Downloading lunchmoney: {start} through {end}");
        let executable = lunchmoney_cli.to_owned();
        let url = format!("cli://lunchmoney/transactions?start_date={start}&end_date={end}");
        let bytes =
            tokio::task::spawn_blocking(move || lunchmoney_export(&executable, &start, &end))
                .await??;
        return save_snapshot(root, r, region, vec![("input.json", url, bytes)]);
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent("ultrafinance-source-downloader")
        .build()?;
    let mut downloaded = Vec::new();
    for (name, url) in files {
        eprintln!("Downloading {}: {name}", r.source.name());
        let bytes = fetch(&client, &url)
            .await
            .with_context(|| format!("download failed: {url}"))?;
        downloaded.push((name, url, bytes));
    }
    save_snapshot(root, r, region, downloaded)
}
// Gregorian civil date from UTC epoch days, then the previous calendar year.
fn last_year(now: std::time::SystemTime) -> (String, String) {
    let days = now
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before Unix epoch")
        .as_secs()
        / 86400;
    let z = days as i64 + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let previous = year - 1;
    let leap = previous % 4 == 0 && (previous % 100 != 0 || previous % 400 == 0);
    let start_day = if month == 2 && day == 29 && !leap {
        28
    } else {
        day
    };
    (
        format!("{previous:04}-{month:02}-{start_day:02}"),
        format!("{year:04}-{month:02}-{day:02}"),
    )
}
fn lunchmoney_export(executable: &Path, start: &str, end: &str) -> Result<Vec<u8>> {
    let output = tempfile::tempfile()?;
    let status = std::process::Command::new(executable)
        .args([
            "--output",
            "json",
            "transactions",
            "list",
            "--start-date",
            start,
            "--end-date",
            end,
            "--all",
            "--limit",
            "1000",
            "--include-metadata",
            "--include-split-parents",
            "--include-group-children",
            "--exclude-pending",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(std::process::Stdio::null())
        .status()
        .context("could not run lunchmoney; install lunchmoney-cli or set --lunchmoney-cli PATH")?;
    if !status.success() {
        bail!(
            "lunchmoney export failed ({status}); check `lunchmoney auth status` and run `lunchmoney transactions list` to diagnose"
        );
    }
    if output.metadata()?.len() > 64 * 1024 * 1024 {
        bail!("Lunch Money export exceeds 64 MiB");
    }
    use std::io::{Read, Seek};
    let mut output = output;
    output.rewind()?;
    let mut bytes = Vec::new();
    output.read_to_end(&mut bytes)?;
    Ok(bytes)
}
fn save_snapshot(
    root: &Path,
    r: &Registration,
    region: &str,
    files: Vec<(&str, String, Vec<u8>)>,
) -> Result<PathBuf> {
    let parent = cache_path(root, r.source, region)?;
    std::fs::create_dir_all(&parent)?;
    let staging = tempfile::tempdir_in(&parent)?;
    let mut entries = Vec::new();
    for (name, url, bytes) in files {
        validate_file(r.source, name, &bytes)?;
        std::fs::write(staging.path().join(name), &bytes)?;
        entries.push(json!({"file": name, "url": url, "bytes": bytes.len(), "fingerprint": ultrafinance_core::eval::fingerprint(&bytes)}));
    }
    let snapshot = format!(
        "snapshot-{}",
        ultrafinance_core::eval::fingerprint(&serde_json::to_vec(&entries)?)
            .replace("fnv1a64:", "")
    );
    let path = parent.join(&snapshot);
    if path.exists() {
        // Verify reused snapshots rather than trusting the pointer or directory name.
        for entry in &entries {
            let name = entry["file"].as_str().unwrap();
            if std::fs::read(path.join(name))? != std::fs::read(staging.path().join(name))? {
                bail!("cached snapshot differs from downloaded contents: {name}");
            }
        }
    } else {
        std::fs::write(
            staging.path().join("download.json"),
            serde_json::to_vec_pretty(
                &json!({"source": metadata(r), "region": region, "files": entries}),
            )?,
        )?;
        std::fs::rename(staging.path(), &path)?;
    }
    atomic_write(&parent.join("latest"), snapshot.as_bytes())?;
    Ok(path)
}
fn save_foursquare_snapshot(
    root: &Path,
    r: &Registration,
    region: &str,
    input: &Path,
    url: String,
) -> Result<PathBuf> {
    let parent = cache_path(root, r.source, region)?;
    std::fs::create_dir_all(&parent)?;
    let staging = tempfile::tempdir_in(&parent)?;
    let copied = staging.path().join("input.csv");
    std::fs::copy(input, &copied)?;
    let mut reader = csv::Reader::from_path(&copied)?;
    if !reader.headers()?.iter().any(|h| h == "fsq_place_id") {
        bail!("download is missing CSV column fsq_place_id");
    }
    for row in reader.records() {
        row?;
    }
    let fingerprint = ultrafinance_core::eval::fingerprint_reader(std::fs::File::open(&copied)?)?;
    let entries = json!([{"file":"input.csv","url":url,"bytes":std::fs::metadata(&copied)?.len(),"fingerprint":fingerprint}]);
    let snapshot = format!(
        "snapshot-{}",
        ultrafinance_core::eval::fingerprint(&serde_json::to_vec(&entries)?)
            .replace("fnv1a64:", "")
    );
    let path = parent.join(&snapshot);
    if path.exists() {
        if ultrafinance_core::eval::fingerprint_reader(std::fs::File::open(
            path.join("input.csv"),
        )?)? != fingerprint
        {
            bail!("cached snapshot differs from downloaded contents: input.csv");
        }
    } else {
        std::fs::write(
            staging.path().join("download.json"),
            serde_json::to_vec_pretty(
                &json!({"source":metadata(r),"region":region,"files":entries}),
            )?,
        )?;
        std::fs::rename(staging.path(), &path)?;
    }
    atomic_write(&parent.join("latest"), snapshot.as_bytes())?;
    Ok(path)
}

fn validate_file(source: Source, name: &str, bytes: &[u8]) -> Result<()> {
    if matches!(source, Source::Foursquare) && name == "examples.json" {
        let data: Value = serde_json::from_slice(bytes)?;
        data["brands"]
            .as_array()
            .context("Foursquare brand review must contain a brands array")?;
        return Ok(());
    }
    if matches!(source, Source::MerchantStudio | Source::LunchMoney) {
        let data: Value = serde_json::from_slice(bytes)?;
        let key = if matches!(source, Source::LunchMoney) {
            if data["has_more"].as_bool() == Some(true) {
                bail!("incomplete Lunch Money export: pagination remains");
            }
            "transactions"
        } else if name == "examples.json" {
            "descriptors"
        } else {
            "merchants"
        };
        data[key]
            .as_array()
            .context("download is missing source rows")?;
    } else {
        let text = std::str::from_utf8(bytes)?;
        let mut reader = csv::Reader::from_reader(text.trim_start_matches('\u{feff}').as_bytes());
        let required = match source {
            Source::Foursquare => "fsq_place_id",
            Source::OpenEnrichment => "transaction_text_examples",
            Source::DoDataThings => "description",
            Source::MoneyVis => "Transaction Description",
            Source::BusinessTransactions => "transaction_string",
            Source::MerchantStudio | Source::LunchMoney => unreachable!(),
        };
        if !reader.headers()?.iter().any(|h| h == required) {
            bail!("download is missing CSV column {required}");
        }
        for row in reader.records() {
            row?;
        }
    }
    Ok(())
}
fn snapshot_files(path: &Path) -> Result<(PathBuf, Option<PathBuf>)> {
    let metadata: Value = serde_json::from_slice(&std::fs::read(path.join("download.json"))?)?;
    let mut input = None;
    let mut examples = None;
    for entry in metadata["files"]
        .as_array()
        .context("invalid download metadata")?
    {
        let name = entry["file"]
            .as_str()
            .context("missing download filename")?;
        if !["input.json", "input.csv", "examples.json"].contains(&name) {
            bail!("invalid download filename");
        }
        let file = path.join(name);
        let fingerprint=ultrafinance_core::eval::fingerprint_reader(std::fs::File::open(&file)?)?;
        if json!(fingerprint) != entry["fingerprint"] {
            bail!("downloaded file changed: {name}");
        }
        if name == "examples.json" {
            examples = Some(file);
        } else {
            input = Some(file);
        }
    }
    Ok((input.context("snapshot has no input file")?, examples))
}
fn raw_rows(path: &Path, examples: bool, limit: usize, offset: usize) -> Result<Value> {
    let text = std::fs::read_to_string(path)?;
    if path.extension().is_some_and(|x| x == "json") {
        let data: Value = serde_json::from_str(&text)?;
        let rows = data[if data["brands"].is_array() {
            "brands"
        } else if examples {
            "descriptors"
        } else if data["transactions"].is_array() {
            "transactions"
        } else {
            "merchants"
        }]
        .as_array()
        .context("missing source rows")?;
        Ok(
            json!({"total": rows.len(), "offset": offset, "rows": rows.iter().skip(offset).take(limit).collect::<Vec<_>>()}),
        )
    } else {
        let mut reader = csv::Reader::from_reader(text.trim_start_matches('\u{feff}').as_bytes());
        let headers = reader.headers()?.clone();
        let rows = reader
            .records()
            .skip(offset)
            .take(limit)
            .map(|row| {
                Ok(Value::Object(
                    headers
                        .iter()
                        .zip(row?.iter())
                        .map(|(key, value)| (key.into(), json!(value)))
                        .collect(),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(json!({"offset": offset, "rows": rows}))
    }
}

pub async fn run(args: Args, database_url: Option<&str>) -> Result<()> {
    ultrafinance_core::set_import_verbose(args.verbose);
    let root = &args.cache_dir;
    let result = match args.command {
        Command::Eval {
            source,
            region,
            refresh,
            offline,
            datasets_dir,
            mode,
            details,
            limit,
            output,
            model,
            threshold,
        } => {
            let r = registry(source);
            if matches!(r.source, Source::Foursquare) {
                bail!(
                    "Foursquare supplies merchant knowledge, not transaction evaluation samples; evaluate an independent transaction suite after importing"
                );
            }
            let parent = cache_path(root, r.source, &region)?;
            let snapshot = if refresh || (!offline && !parent.join("latest").exists()) {
                download(
                    root,
                    &r,
                    &region,
                    &args.lunchmoney_cli,
                    &args.duckdb_cli,
                    args.foursquare_limit,
                )
                .await?
            } else {
                latest(root, r.source, &region)?
            };
            let (input, examples) = snapshot_files(&snapshot)?;
            let contents = std::fs::read_to_string(input)?;
            let examples = examples.map(std::fs::read_to_string).transpose()?;
            if matches!(r.source, Source::Foursquare) {
                bail!(
                    "Foursquare supplies merchant knowledge, not transaction evaluation samples; evaluate an independent transaction suite after importing"
                );
            }
            let bundle = datasets::prepare(r.source, &contents, examples.as_deref(), &region)?;
            let path = bundle.save(&datasets_dir)?;
            let (mode, mode_name) = match mode {
                EvalMode::Search => (ultrafinance_core::eval::Mode::Search, "search"),
                EvalMode::Enrich => (ultrafinance_core::eval::Mode::Enrich, "enrich"),
            };
            let output = output.unwrap_or_else(|| {
                PathBuf::from(format!(
                    "evals/reports/{}-{region}-holdout-{mode_name}.json",
                    r.source.name()
                ))
            });
            eprintln!(
                "Evaluating {} holdout from {}",
                r.source.name(),
                path.display()
            );
            if bundle.manifest.labeled_holdout == 0 {
                eprintln!(
                    "This source has no merchant labels: results measure coverage, not accuracy."
                );
            }
            batch::run_file(batch::FileOptions {
                file: path.join("holdout.jsonl"),
                samples: true,
                details,
                limit,
                output: Some(output),
                database_url: database_url.map(str::to_owned),
                mode,
                model,
                threshold,
            })
            .await?;
            return Ok(());
        }
        Command::List { json: as_json } => {
            let registrations = [
                DatasetSource::MerchantStudio,
                DatasetSource::OpenEnrichment,
                DatasetSource::Dodatathings,
                DatasetSource::Moneyvis,
                DatasetSource::Lunchmoney,
                DatasetSource::BusinessTransactions,
                DatasetSource::Foursquare,
            ]
            .map(registry);
            if !as_json {
                let mut table = comfy_table::Table::new();
                table.set_header(["Source", "Use", "Download"]);
                for r in registrations {
                    table.add_row([
                        r.source.name(),
                        r.purpose,
                        if matches!(r.source, Source::Foursquare) {
                            "automatic-authenticated"
                        } else {
                            "automatic"
                        },
                    ]);
                }
                println!("{table}");
                return Ok(());
            }
            json!(registrations.iter().map(metadata).collect::<Vec<_>>())
        }
        Command::Show { source, region } => {
            let r = registry(source);
            let files = urls(r.source, &region)?;
            let mut info = metadata(&r);
            info["downloads"] = json!(
                files
                    .iter()
                    .map(|(name, url)| json!({"file":name,"url":url}))
                    .collect::<Vec<_>>()
            );
            let parent = cache_path(root, r.source, &region)?;
            info["snapshot"] = if parent.join("latest").exists() {
                json!(latest(root, r.source, &region)?)
            } else {
                Value::Null
            };
            info
        }
        Command::Download { source, region } => {
            let r = registry(source);
            json!({"snapshot":download(root, &r, &region, &args.lunchmoney_cli, &args.duckdb_cli, args.foursquare_limit).await?})
        }
        Command::Raw {
            source,
            region,
            limit,
            offset,
            examples,
        } => {
            let r = registry(source);
            let (input, example_file) = snapshot_files(&latest(root, r.source, &region)?)?;
            let file = if examples {
                example_file.context("this source has no examples file")?
            } else {
                input
            };
            raw_rows(&file, examples, limit as usize, offset)?
        }
        Command::Records {
            source,
            external_id,
            limit,
            offset,
        } => {
            json!(MerchantStore::configured(database_url)?.source_records(
                &source,
                external_id.as_deref(),
                limit as usize,
                offset
            )?)
        }
        Command::Import {
            source,
            region,
            offline,
            input,
            examples,
            output,
            dry_run,
            evaluation,
            dedupe,
        } => {
            let r = registry(source);
            urls(r.source, &region)?;
            if matches!(r.source, Source::BusinessTransactions) && !dry_run && !evaluation {
                bail!(
                    "BusinessTransactions is a synthetic reference catalog; use --dry-run to prepare it, or --evaluation with a separate evaluation database"
                );
            }
            if examples.is_some() && input.is_none() && !matches!(r.source, Source::Foursquare) {
                bail!("--examples without --input is supported only for Foursquare");
            }
            let (input, examples) = if let (Source::Foursquare,Some(input),None)=(r.source,input.as_ref(),examples.as_ref()) {
                snapshot_files(&save_foursquare_snapshot(root,&r,&region,input,format!("local:{}",input.display()))?)?
            } else if let Some(input) = input {
                let name = if matches!(r.source, Source::MerchantStudio | Source::LunchMoney) {
                    "input.json"
                } else {
                    "input.csv"
                };
                let mut files = vec![(
                    name,
                    format!("local:{}", input.display()),
                    std::fs::read(&input)?,
                )];
                if let Some(examples) = examples {
                    files.push((
                        "examples.json",
                        format!("local:{}", examples.display()),
                        std::fs::read(examples)?,
                    ));
                }
                snapshot_files(&save_snapshot(root, &r, &region, files)?)?
            } else {
                let path = if offline {
                    latest(root, r.source, &region)?
                } else {
                    download(
                        root,
                        &r,
                        &region,
                        &args.lunchmoney_cli,
                        &args.duckdb_cli,
                        args.foursquare_limit,
                    )
                    .await?
                };
                if let Some(review) = examples {
                    let (input, _) = snapshot_files(&path)?;
                    let metadata: Value =
                        serde_json::from_slice(&std::fs::read(path.join("download.json"))?)?;
                    let url = metadata["files"][0]["url"]
                        .as_str()
                        .context("snapshot missing source URL")?
                        .to_owned();
                    snapshot_files(&save_snapshot(
                        root,
                        &r,
                        &region,
                        vec![
                            ("input.csv", url, std::fs::read(input)?),
                            (
                                "examples.json",
                                format!("local:{}", review.display()),
                                std::fs::read(review)?,
                            ),
                        ],
                    )?)?
                } else {
                    snapshot_files(&path)?
                }
            };
            let (path,manifest)=if matches!(r.source,Source::Foursquare) && examples.is_none() {
                datasets::prepare_foursquare_file(&input,&output,&region)?
            } else {
                let contents = std::fs::read_to_string(input)?;
            let examples = examples.map(std::fs::read_to_string).transpose()?;
            let (path, manifest) = if let Some(cached) =
                datasets::cached_bundle(&output, r.source, &contents, examples.as_deref(), &region)?
            {
                eprintln!(
                    "Import: reusing prepared {} merchant bundle",
                    r.source.name()
                );
                cached
            } else {
                eprintln!("Import: preparing {} merchant bundle", r.source.name());
                let bundle = datasets::prepare(r.source, &contents, examples.as_deref(), &region)?;
                eprintln!(
                    "Import: saving bundle ({} source records)",
                    bundle.records.len()
                );
                let path = bundle.save(&output)?;
                (path, bundle.manifest)
            };
                (path,manifest)
            };
            let mut report = json!({"bundle": path, "manifest": manifest, "dry_run": dry_run});
            // Limit the selected records, leaving the complete cached snapshot
            // and prepared bundle intact for later uncapped imports.
            eprintln!("Import: reading prepared source records");
            if dry_run {
                let (available, selected) = datasets::stream_records(&path.join("knowledge.json"), dedupe.limit, dedupe.chunk_size as usize, |_| Ok(()))?;
                report["selection"] = dedupe.selection(available, selected);
            } else {
                let (result, available, selected) = ultrafinance_core::dedupe::import_file(
                    MerchantStore::configured(database_url)?, path.join("knowledge.json"), dedupe.limit, dedupe.options(),
                ).await?;
                report["selection"] = dedupe.selection(available, selected);
                report["dry_run"] = json!(result.dedupe.dry_run);
                report["delta"] = json!(result.delta);
                report["dedupe"] = json!(result.dedupe);
            }
            report
        }
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rolling_year_handles_leap_days_and_calendar_boundaries() {
        for (epoch, start, end) in [
            (1709164800, "2023-02-28", "2024-02-29"),
            (1767225600, "2025-01-01", "2026-01-01"),
            (1791504000, "2025-10-09", "2026-10-09"),
        ] {
            assert_eq!(
                last_year(std::time::UNIX_EPOCH + Duration::from_secs(epoch)),
                (start.into(), end.into())
            );
        }
    }
    #[test]
    fn regions_cannot_escape_cache_or_change_upstream_path() {
        for region in ["../us", "US", "us/../../", "", "global?x=1"] {
            assert!(urls(Source::OpenEnrichment, region).is_err());
        }
        assert!(
            urls(Source::OpenEnrichment, "uk").unwrap()[0]
                .1
                .contains("/uk/merchants.csv")
        );
        assert!(urls(Source::MerchantStudio, "us").is_err());
    }
    #[test]
    fn raw_csv_preserves_fields_and_paginates() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("input.csv");
        std::fs::write(
            &file,
            "name,description\nAlpha,\"PAYMENT $12, STORE 2\"\nBeta,OTHER\n",
        )
        .unwrap();
        assert_eq!(
            raw_rows(&file, false, 1, 0).unwrap()["rows"][0]["description"],
            "PAYMENT $12, STORE 2"
        );
        assert_eq!(
            raw_rows(&file, false, 1, 1).unwrap()["rows"][0]["name"],
            "Beta"
        );
    }
    #[test]
    fn snapshot_corruption_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("input.csv"), "changed").unwrap();
        std::fs::write(dir.path().join("download.json"), serde_json::to_vec(&json!({"files":[{"file":"input.csv", "fingerprint": ultrafinance_core::eval::fingerprint(b"original")}]})).unwrap()).unwrap();
        assert!(snapshot_files(dir.path()).is_err());
    }
    #[test]
    fn snapshots_are_reused_and_invalid_updates_leave_latest_intact() {
        let root = tempfile::tempdir().unwrap();
        let r = registry(DatasetSource::Dodatathings);
        let files = || {
            vec![(
                "input.csv",
                "https://example.test/source.csv".into(),
                b"description,category\nPAYMENT,Other\n".to_vec(),
            )]
        };
        let first = save_snapshot(root.path(), &r, "global", files()).unwrap();
        assert_eq!(
            save_snapshot(root.path(), &r, "global", files()).unwrap(),
            first
        );
        assert!(
            save_snapshot(
                root.path(),
                &r,
                "global",
                vec![(
                    "input.csv",
                    "https://example.test/source.csv".into(),
                    b"<html>Error</html>".to_vec()
                )]
            )
            .is_err()
        );
        assert_eq!(latest(root.path(), r.source, "global").unwrap(), first);
        assert!(snapshot_files(&first).is_ok());
    }
    #[tokio::test]
    async fn downloader_rejects_http_errors_empty_and_oversized_responses() {
        use std::io::Read;
        for response in [
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 67108865\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok",
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/data", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut buf = [0; 4096];
                let read = stream.read(&mut buf).unwrap();
                assert!(read > 0);
                stream.write_all(response.as_bytes()).unwrap();
            });
            let result = fetch(
                &reqwest::Client::builder().no_proxy().build().unwrap(),
                &url,
            )
            .await;
            if response.ends_with("ok") {
                assert_eq!(result.unwrap(), b"ok");
            } else {
                assert!(result.is_err());
            }
            server.join().unwrap();
        }
    }
}
