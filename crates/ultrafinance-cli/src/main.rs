mod batch;
mod build_metadata;
mod infra;
mod output;
use anyhow::{Context, Result, bail};
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use serde_json::Map;
use std::{
    env,
    io::{self, Read},
    path::PathBuf,
    time::Duration,
};
use ultrafinance_core::batch::{BatchEnrichRequest, BatchEnrichResponse, BatchItemResult};
use ultrafinance_core::{EnrichRequest, Enricher, Merchant, load_catalog, store::MerchantStore};

#[derive(Parser)]
#[command(
    name = "ultrafinance",
    version = env!("ULTRAFINANCE_CLI_VERSION"),
    about = "Test merchant enrichment locally"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// PostgreSQL connection URL. Prefer the environment variable to keep credentials out of shell history.
    #[arg(
        long,
        global = true,
        env = "ULTRAFINANCE_DATABASE_URL",
        hide_env_values = true
    )]
    database_url: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Deploy the workspace, inspect Lambda logs, or open the production CLI shell.
    Infra(infra::InfraArgs),
    /// Preview competing merchant/location interpretations without provider calls.
    Interpret { description: String },
    /// Inspect, verify or revoke context-scoped remembered descriptor resolutions.
    Resolutions {
        #[command(subcommand)]
        command: ResolutionCommand,
    },
    /// Initialize or upgrade the PostgreSQL schema.
    Database {
        #[command(subcommand)]
        command: DatabaseCommand,
    },
    /// Inspect private enrichment history (newest first).
    Logs {
        /// Print full records as JSON instead of a summary table.
        #[arg(long)]
        json: bool,
        #[arg(long, value_parser = ["started", "matched", "unresolved", "error"])]
        status: Option<String>,
        #[arg(long)]
        merchant_id: Option<String>,
        #[arg(long, default_value = "50")]
        limit: usize,
        #[arg(long, default_value = "0")]
        offset: usize,
    },
    /// Enrich a description or a complete JSON request without starting the API.
    Enrich(Box<EnrichArgs>),
    /// Enrich up to 100 transactions using shared provider batches.
    EnrichBatch(BatchEnrichArgs),
    /// Benchmark labeled suites or measure coverage of dataset samples.
    Eval {
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        file: Option<PathBuf>,
        /// Run all suites and the latest holdout snapshot from each dataset.
        #[arg(long, conflicts_with = "samples")]
        all: bool,
        #[arg(long, default_value = "data/datasets", requires = "all")]
        datasets_dir: PathBuf,
        #[arg(long, default_value = "evals", requires = "all")]
        suites_dir: PathBuf,
        /// Read dataset JSONL samples, including ones without merchant labels.
        #[arg(long)]
        samples: bool,
        /// Evaluate only the first N cases (useful before a provider-backed run).
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
        limit: Option<u32>,
        #[arg(long, value_enum, default_value = "search")]
        mode: EvalMode,
        /// Report file, or parent report directory when using --all.
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long, env = "JEV_MODEL", default_value = "jev-latest")]
        model: String,
        #[arg(long, env = "ULTRAFINANCE_MATCH_THRESHOLD", default_value = "0.95")]
        threshold: f64,
    },
    /// Prepare repeatable dataset snapshots, development data, and evals.
    Datasets {
        #[command(subcommand)]
        command: DatasetCommand,
    },
    /// Import reviewed merchant outlets or inspect a merchant's locations.
    Locations {
        #[command(subcommand)]
        command: LocationCommand,
    },
    /// Maintain and search the local merchant database.
    Merchants {
        #[command(subcommand)]
        command: MerchantCommand,
    },
}

#[derive(Subcommand)]
enum LocationCommand {
    /// Import reviewed outlet JSON. Import the referenced merchants first.
    Import { file: PathBuf },
    /// Evaluate location-only labels offline with geography and outlet accuracy.
    Eval { file: PathBuf },
    /// List catalog outlets for a local merchant ID, including provenance.
    List { merchant_id: String },
}

#[derive(Subcommand)]
enum DatabaseCommand {
    /// Apply PostgreSQL schema migrations (safe to repeat).
    Init,
}

#[derive(Subcommand)]
enum ResolutionCommand {
    /// List remembered mappings and their evidence; no provider calls.
    List {
        #[arg(long, default_value = "50")]
        limit: usize,
    },
    /// Verify an existing resolution after independently checking its merchant and context.
    Confirm {
        id: String,
        #[arg(long)]
        evidence: String,
    },
    /// Remove a mapping; its merchant and original history remain available.
    Revoke { id: String },
}

#[derive(Subcommand)]
enum DatasetCommand {
    /// Convert a downloaded dataset; applying knowledge is an explicit separate command.
    Import {
        #[arg(long, value_enum)]
        source: DatasetSource,
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        examples: Option<PathBuf>,
        #[arg(long, default_value = "global")]
        region: String,
        #[arg(long, default_value = "data/datasets")]
        output: PathBuf,
    },
    /// Apply a prepared knowledge.json batch to the merchant database.
    Apply { file: PathBuf },
    /// Export manually labeled JSONL samples into a merchant eval suite.
    ExportEval {
        file: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
}
#[derive(Clone, clap::ValueEnum)]
enum DatasetSource {
    MerchantStudio,
    OpenEnrichment,
    Dodatathings,
    Moneyvis,
    /// Synthetic merchant-labeled descriptions; names-only evaluation catalog.
    BusinessTransactions,
}

#[derive(Subcommand)]
enum MerchantCommand {
    /// Find duplicates with Jev and automatically merge supported groups.
    Dedupe {
        /// Evaluate and report decisions without merging merchants.
        #[arg(long)]
        dry_run: bool,
        #[arg(long, env = "JEV_MODEL", default_value = "jev-latest")]
        model: String,
        /// Require both same-merchant probability and confidence to reach this value.
        #[arg(long, default_value = "0.98")]
        threshold: f64,
        /// Fail before provider calls if the complete scan exceeds this pair budget.
        #[arg(long, default_value = "10000", value_parser = clap::value_parser!(u32).range(1..))]
        max_pairs: u32,
        /// Also save the JSON report to a file.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Show catalog totals and breakdowns by imported source and known market.
    Stats {
        /// Print statistics as JSON for scripts.
        #[arg(long)]
        json: bool,
    },
    /// Add a verified merchant, or replace one by supplying its existing ID.
    Add {
        #[arg(long)]
        name: String,
        /// Declare a known operating market (repeat for multiple countries).
        #[arg(long = "market")]
        markets: Vec<String>,
        #[arg(long)]
        website: Option<String>,
        /// Public HTTP(S) URL of a verified merchant brand logo.
        #[arg(long)]
        logo_url: Option<String>,
        /// Logo origin, such as an official website or dataset name.
        #[arg(long, requires = "logo_url")]
        logo_source: Option<String>,
        #[arg(long = "alias")]
        aliases: Vec<String>,
        #[arg(long = "source")]
        sources: Vec<String>,
        #[arg(long)]
        id: Option<String>,
    },
    /// List stored merchants alphabetically, without calling Jev.
    #[command(alias = "ls")]
    List {
        /// Filter by known market evidence; merchants without that evidence are excluded.
        #[arg(long = "market")]
        market: Option<String>,
        #[arg(long, default_value = "50")]
        limit: usize,
        #[arg(long, default_value = "0")]
        offset: usize,
        /// Print the complete page as JSON for scripts.
        #[arg(long)]
        json: bool,
    },
    /// Show exact and fuzzy candidates without calling Jev.
    Search {
        description: String,
        /// Transaction country. Prefers known markets without excluding other candidates.
        #[arg(long)]
        country: Option<String>,
        #[arg(long, default_value = "10")]
        limit: usize,
    },
    /// Import a JSON merchant catalog, updating records with the same ID.
    Import {
        file: PathBuf,
        /// Dataset format: native UltraFinance JSON or Merchant Studio.
        #[arg(long, value_enum, default_value = "native")]
        format: ImportFormat,
        /// Namespace for native catalog IDs.
        #[arg(long, default_value = "catalog")]
        source: String,
    },
    /// Link an external record to an existing local merchant (no automatic name merging).
    Link {
        #[arg(long)]
        source: String,
        #[arg(long)]
        external_id: String,
        #[arg(long)]
        merchant_id: String,
    },
}

#[derive(Clone, clap::ValueEnum)]
enum EvalMode {
    Search,
    Enrich,
}

#[derive(Clone, clap::ValueEnum)]
enum ImportFormat {
    Native,
    MerchantStudio,
}

#[derive(Args)]
struct EnrichArgs {
    /// Raw bank transaction description.
    #[arg(required_unless_present = "input", conflicts_with = "input")]
    description: Option<String>,
    /// Read a complete JSON request from a file, or use - for stdin.
    #[arg(long, conflicts_with_all = ["description", "amount", "currency", "date", "country", "location", "extra"])]
    input: Option<PathBuf>,
    /// Decimal amount, supplied as a string.
    #[arg(long, allow_hyphen_values = true)]
    amount: Option<String>,
    #[arg(long)]
    currency: Option<String>,
    /// Transaction date in YYYY-MM-DD form.
    #[arg(long)]
    date: Option<String>,
    #[arg(long)]
    country: Option<String>,
    /// Structured transaction geography as a JSON object.
    #[arg(long)]
    location: Option<String>,
    /// Additional evidence as a JSON object.
    #[arg(long)]
    extra: Option<String>,
    /// Use an isolated PostgreSQL catalog loaded from JSON for this request.
    #[arg(long, env = "ULTRAFINANCE_MERCHANTS")]
    merchants: Option<PathBuf>,
    #[arg(long, env = "JEV_MODEL", default_value = "jev-latest")]
    model: String,
    /// Minimum chosen probability and model confidence; provisional, not measured accuracy.
    #[arg(long, env = "ULTRAFINANCE_MATCH_THRESHOLD", default_value = "0.95")]
    threshold: f64,
    /// Validate and print the request without reading a catalog or calling Jev.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct BatchEnrichArgs {
    /// JSON object containing transactions; use - for stdin (maximum 1 MiB).
    #[arg(long)]
    input: PathBuf,
    #[arg(long, env = "JEV_MODEL", default_value = "jev-latest")]
    model: String,
    #[arg(long, env = "ULTRAFINANCE_MATCH_THRESHOLD", default_value = "0.95")]
    threshold: f64,
    #[arg(long)]
    dry_run: bool,
}
fn read_batch_request(path: &std::path::Path) -> Result<BatchEnrichRequest> {
    let reader: Box<dyn Read> = if path.as_os_str() == "-" {
        Box::new(io::stdin())
    } else {
        Box::new(
            std::fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?,
        )
    };
    let mut bytes = Vec::new();
    reader.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        bail!("request exceeds 1 MiB");
    }
    let request: BatchEnrichRequest =
        serde_json::from_slice(&bytes).context("invalid batch request JSON")?;
    request.validate()?;
    Ok(request)
}

fn read_request(args: &EnrichArgs) -> Result<EnrichRequest> {
    let request = if let Some(path) = &args.input {
        // Match the API's 64 KiB input limit, including when stdin is used.
        let reader: Box<dyn Read> = if path.as_os_str() == "-" {
            Box::new(io::stdin())
        } else {
            Box::new(
                std::fs::File::open(path)
                    .with_context(|| format!("cannot open {}", path.display()))?,
            )
        };
        let mut bytes = Vec::new();
        reader
            .take(65537)
            .read_to_end(&mut bytes)
            .context("cannot read request")?;
        if bytes.len() > 65536 {
            bail!("request exceeds 64 KiB");
        }
        serde_json::from_slice(&bytes).context("invalid request JSON")?
    } else {
        EnrichRequest {
            description: args
                .description
                .clone()
                .context("description or --input is required")?,
            amount: args.amount.clone(),
            currency: args.currency.clone(),
            date: args.date.clone(),
            country: args.country.clone(),
            location: args
                .location
                .as_ref()
                .map(|value| {
                    serde_json::from_str(value).context("--location must be a location object")
                })
                .transpose()?,
            extra: match &args.extra {
                Some(extra) => serde_json::from_str::<Map<String, serde_json::Value>>(extra)
                    .context("--extra must be a JSON object")?,
                None => Map::new(),
            },
        }
    };
    if serde_json::to_vec(&request)?.len() > 65536 {
        bail!("request exceeds 64 KiB");
    }
    request.validate()?;
    Ok(request)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Check for missing subcommands after parsing so global flags and environment
    // defaults do not prevent command groups from showing their help.
    fn help_on_missing_subcommand(command: clap::Command) -> clap::Command {
        if command.has_subcommands() {
            command
                .subcommand_required(false)
                .arg_required_else_help(false)
                .mut_subcommands(help_on_missing_subcommand)
        } else {
            command
        }
    }
    let mut command = help_on_missing_subcommand(Cli::command());
    let matches = command.get_matches_mut();
    let mut current_matches = &matches;
    while let Some((name, submatches)) = current_matches.subcommand() {
        command = command.find_subcommand_mut(name).unwrap().clone();
        current_matches = submatches;
    }
    if command.has_subcommands() {
        command.print_help()?;
        println!();
        return Ok(());
    }
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    match cli.command {
        Command::Infra(args) => infra::run(args)?,
        Command::Interpret { description } => {
            let request: EnrichRequest =
                serde_json::from_value(serde_json::json!({"description":description}))?;
            request.validate()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&ultrafinance_core::interpretation::interpret(
                    &request
                ))?
            );
        }
        Command::Resolutions { command } => {
            let store = MerchantStore::configured(cli.database_url.as_deref())?;
            match command {
                ResolutionCommand::List { limit } => println!(
                    "{}",
                    serde_json::to_string_pretty(&store.resolutions(None, limit)?)?
                ),
                ResolutionCommand::Confirm { id, evidence } => {
                    let mut mapping = store
                        .resolutions(Some(&id), 1)?
                        .into_iter()
                        .next()
                        .context("resolution not found")?;
                    mapping.verified = true;
                    mapping.evidence = Some(evidence);
                    if !store.save_resolution(&mapping)? {
                        bail!(
                            "resolution schema missing; run database init with a schema-owner connection"
                        );
                    }
                    println!("Verified {}", mapping.id);
                }
                ResolutionCommand::Revoke { id } => {
                    if !store.revoke_resolution(&id)? {
                        bail!("resolution not found");
                    }
                    println!("Revoked {id}");
                }
            }
        }
        Command::Logs {
            json,
            status,
            merchant_id,
            limit,
            offset,
        } => {
            let store = MerchantStore::configured(cli.database_url.as_deref())?;
            output::enrichment_logs(
                &store.enrichment_logs(status.as_deref(), merchant_id.as_deref(), limit, offset)?,
                json,
                offset,
            )?;
        }
        Command::Database { command } => {
            let url = cli
                .database_url
                .as_deref()
                .unwrap_or(ultrafinance_core::store::LOCAL_DATABASE_URL);
            match command {
                DatabaseCommand::Init => {
                    MerchantStore::initialize_postgres(url)?;
                    println!("PostgreSQL schema is ready");
                }
            }
        }
        Command::Locations { command } => {
            let store = MerchantStore::configured(cli.database_url.as_deref())?;
            match command {
                LocationCommand::Import { file } => {
                    let records: Vec<ultrafinance_core::location::LocationRecord> =
                        serde_json::from_str(&std::fs::read_to_string(file)?)?;
                    store.import_locations(&records)?;
                    println!("{}", serde_json::json!({"imported":records.len()}));
                }
                LocationCommand::Eval { file } => {
                    let suite = serde_json::from_str(&std::fs::read_to_string(file)?)?;
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&ultrafinance_core::location::evaluate(
                            &store, suite
                        )?)?
                    );
                }
                LocationCommand::List { merchant_id } => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&store.locations(&merchant_id)?)?
                    );
                }
            }
        }
        Command::Datasets { command } => match command {
            DatasetCommand::Import {
                source,
                input,
                examples,
                region,
                output,
            } => {
                let source = match source {
                    DatasetSource::MerchantStudio => {
                        ultrafinance_core::datasets::Source::MerchantStudio
                    }
                    DatasetSource::OpenEnrichment => {
                        ultrafinance_core::datasets::Source::OpenEnrichment
                    }
                    DatasetSource::Dodatathings => {
                        ultrafinance_core::datasets::Source::DoDataThings
                    }
                    DatasetSource::Moneyvis => ultrafinance_core::datasets::Source::MoneyVis,
                    DatasetSource::BusinessTransactions => {
                        ultrafinance_core::datasets::Source::BusinessTransactions
                    }
                };
                let contents = std::fs::read_to_string(&input)?;
                let examples = examples.map(std::fs::read_to_string).transpose()?;
                let bundle = ultrafinance_core::datasets::prepare(
                    source,
                    &contents,
                    examples.as_deref(),
                    &region,
                )?;
                let path = bundle.save(&output)?;
                println!(
                    "{}",
                    serde_json::json!({"path":path,"manifest":bundle.manifest})
                );
            }
            DatasetCommand::Apply { file } => {
                let records: Vec<ultrafinance_core::store::SourceRecord> =
                    serde_json::from_str(&std::fs::read_to_string(file)?)?;
                MerchantStore::configured(cli.database_url.as_deref())?.import(&records)?;
                println!("{}", serde_json::json!({"imported":records.len()}));
            }
            DatasetCommand::ExportEval { file, output } => {
                let contents = std::fs::read_to_string(file)?;
                let samples: Vec<ultrafinance_core::datasets::Sample> = contents
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .map(serde_json::from_str)
                    .collect::<std::result::Result<_, _>>()?;
                if !samples.iter().any(|s| s.expected.is_some()) {
                    bail!("no merchant labels; add expected labels before export");
                }
                let suite = ultrafinance_core::datasets::eval_suite("manually-labeled", &samples);
                let _: ultrafinance_core::eval::Suite = serde_json::from_value(suite.clone())?;
                std::fs::write(output, serde_json::to_string_pretty(&suite)?)?;
                println!(
                    "{}",
                    serde_json::json!({"cases":suite["cases"].as_array().unwrap().len()})
                );
            }
        },
        Command::Eval {
            file,
            all,
            datasets_dir,
            suites_dir,
            samples,
            limit,
            mode,
            output,
            model,
            threshold,
        } => {
            let mode = match mode {
                EvalMode::Search => ultrafinance_core::eval::Mode::Search,
                EvalMode::Enrich => ultrafinance_core::eval::Mode::Enrich,
            };
            if all {
                batch::run(batch::Options {
                    datasets_dir,
                    suites_dir,
                    output: output.unwrap_or_else(|| PathBuf::from("evals/reports/all")),
                    database_url: cli.database_url,
                    mode,
                    limit,
                    model,
                    threshold,
                })
                .await?;
                return Ok(());
            }
            let file = file.context("eval requires a file or --all")?;
            let contents = batch::contents(&file, samples, limit)?;
            let report = ultrafinance_core::eval::run(
                &contents,
                MerchantStore::configured(cli.database_url.as_deref())?,
                mode,
                env::var("TYPESAFE_API_KEY").ok(),
                model,
                threshold,
            )
            .await?;
            let json = serde_json::to_string_pretty(&report)?;
            if let Some(path) = output {
                if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, &json)?;
                eprintln!("Saved report to {}", path.display());
            } else {
                println!("{json}");
            }
            if report.metrics.labeled_merchants > 0 {
                eprintln!(
                    "{} cases: retrieved expected merchant for {}/{} known cases; {} errors",
                    report.metrics.cases,
                    report.metrics.retrieval_hits,
                    report.metrics.labeled_merchants,
                    report.metrics.errors
                );
            } else {
                eprintln!(
                    "{} cases; {} errors",
                    report.metrics.cases, report.metrics.errors
                );
            }
            if report.metrics.unlabeled > 0 {
                eprintln!(
                    "{} unlabeled cases; candidate coverage {:.1}%",
                    report.metrics.unlabeled,
                    report.metrics.candidate_coverage.unwrap_or(0.0) * 100.0
                );
            }
            if let Some(rate) = report.metrics.match_rate {
                eprintln!(
                    "Match rate {:.1}% · {} matched · {} unresolved · {} errors",
                    rate * 100.0,
                    report.metrics.matches.unwrap_or(0),
                    report.metrics.unresolved.unwrap_or(0),
                    report.metrics.errors
                );
            }
            if let Some(accuracy) = report.metrics.accuracy {
                eprintln!(
                    "Accuracy {:.1}% · match rate {:.1}% · match precision {}",
                    accuracy * 100.0,
                    report.metrics.match_rate.unwrap_or(0.0) * 100.0,
                    report
                        .metrics
                        .match_precision
                        .map(|p| format!("{:.1}%", p * 100.0))
                        .unwrap_or_else(|| "n/a".into())
                );
            }
        }
        Command::Enrich(args) => {
            let request = read_request(&args)?;
            if args.dry_run {
                println!("{}", serde_json::to_string_pretty(&request)?);
                return Ok(());
            }
            let store = if let Some(path) = &args.merchants {
                let store = MerchantStore::temporary_on(
                    cli.database_url
                        .as_deref()
                        .unwrap_or(ultrafinance_core::store::LOCAL_DATABASE_URL),
                )?;
                for merchant in load_catalog(Some(path))? {
                    store.put(&merchant)?;
                }
                store
            } else {
                MerchantStore::configured(cli.database_url.as_deref())?
            };
            let enricher = Enricher::with_store(
                env::var("TYPESAFE_API_KEY").ok(),
                args.model,
                args.threshold,
                store,
            )?;
            let response = tokio::time::timeout(Duration::from_secs(55), enricher.enrich(&request))
                .await
                .context("merchant evaluation timed out")??;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        Command::EnrichBatch(args) => {
            let request = read_batch_request(&args.input)?;
            if args.dry_run {
                for transaction in &request.transactions {
                    transaction.validate()?;
                }
                println!("{}", serde_json::to_string_pretty(&request)?);
                return Ok(());
            }
            let store = MerchantStore::configured(cli.database_url.as_deref())?;
            let enricher = Enricher::with_store(
                env::var("TYPESAFE_API_KEY").ok(),
                args.model,
                args.threshold,
                store,
            )?;
            let outcomes = tokio::time::timeout(
                Duration::from_secs(55),
                enricher.enrich_batch(&request.transactions),
            )
            .await
            .context("merchant batch evaluation timed out")?;
            let mut failed = false;
            let results = request
                .transactions
                .iter()
                .zip(outcomes)
                .map(|(request, outcome)| match outcome {
                    Ok(data) => BatchItemResult::Success {
                        data: Box::new(data),
                    },
                    Err(error) => {
                        failed = true;
                        BatchItemResult::Error {
                            code: if request.validate().is_err() {
                                "invalid_request"
                            } else {
                                "enrichment_failed"
                            }
                            .into(),
                            message: error.to_string(),
                        }
                    }
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&BatchEnrichResponse { results })?
            );
            if failed {
                bail!("some transactions failed; see results above");
            }
        }
        Command::Merchants { command } => {
            let store = MerchantStore::configured(cli.database_url.as_deref())?;
            match command {
                MerchantCommand::Dedupe {
                    dry_run,
                    model,
                    threshold,
                    max_pairs,
                    output,
                } => {
                    let report = ultrafinance_core::dedupe::run(
                        store,
                        env::var("TYPESAFE_API_KEY").ok(),
                        model,
                        threshold,
                        dry_run,
                        max_pairs as usize,
                    )
                    .await?;
                    let json = serde_json::to_string_pretty(&report)?;
                    if let Some(path) = output {
                        std::fs::write(&path, &json)
                            .with_context(|| format!("cannot write {}", path.display()))?;
                    }
                    println!("{json}");
                }
                MerchantCommand::Stats { json } => {
                    output::merchant_stats(&store.stats()?, json)?;
                }
                MerchantCommand::Add {
                    name,
                    mut markets,
                    website,
                    logo_url,
                    logo_source,
                    aliases,
                    sources,
                    id,
                } => {
                    markets.sort();
                    markets.dedup();
                    let merchant = Merchant {
                        id: id.unwrap_or_else(|| format!("mer_{}", uuid::Uuid::new_v4().simple())),
                        name,
                        markets,
                        market_evidence: vec![],
                        website,
                        logo_source: logo_source
                            .or_else(|| logo_url.as_ref().map(|_| "manual".into())),
                        logo_url,
                        aliases,
                        sources,
                    };
                    store.put(&merchant)?;
                    println!("{}", serde_json::to_string_pretty(&merchant)?);
                }
                MerchantCommand::List {
                    market,
                    limit,
                    offset,
                    json,
                } => {
                    let page = store.list(market.as_deref(), limit, offset)?;
                    output::merchant_list(&page, json)?;
                }
                MerchantCommand::Search {
                    description,
                    country,
                    limit,
                } => {
                    if !(1..=254).contains(&limit) {
                        bail!("limit must be between 1 and 254");
                    }
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&store.search(
                            &description,
                            country.as_deref(),
                            limit
                        )?)?
                    );
                }
                MerchantCommand::Import {
                    file,
                    format,
                    source,
                } => {
                    let contents = std::fs::read_to_string(&file)
                        .with_context(|| format!("cannot read {}", file.display()))?;
                    let records = match format {
                        ImportFormat::Native => {
                            ultrafinance_core::import::catalog(&contents, &source)?
                        }
                        ImportFormat::MerchantStudio => {
                            ultrafinance_core::import::merchant_studio(&contents)?
                        }
                    };
                    store.import(&records)?;
                    println!(
                        "{}",
                        serde_json::json!({"imported":records.len(),"attributions":records.first().map(|r|&r.attribution)})
                    );
                }
                MerchantCommand::Link {
                    source,
                    external_id,
                    merchant_id,
                } => {
                    store.link(&source, &external_id, &merchant_id)?;
                    println!(
                        "{}",
                        serde_json::json!({"source":source,"external_id":external_id,"merchant_id":merchant_id})
                    );
                }
            }
        }
    }
    Ok(())
}
