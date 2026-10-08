mod output;
use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde_json::Map;
use std::{
    env,
    io::{self, Read},
    path::PathBuf,
    time::Duration,
};
use ultrafinance_core::{EnrichRequest, Enricher, Merchant, load_catalog, store::MerchantStore};

#[derive(Parser)]
#[command(
    name = "ultrafinance",
    version,
    about = "Test merchant enrichment locally",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// SQLite merchant database.
    #[arg(
        long,
        global = true,
        env = "ULTRAFINANCE_DB",
        default_value = "data/ultrafinance.sqlite"
    )]
    database: PathBuf,
}

#[derive(Subcommand)]
enum Command {
    /// Enrich a description or a complete JSON request without starting the API.
    Enrich(Box<EnrichArgs>),
    /// Benchmark labeled cases. Search mode is offline; enrich mode may call Jev.
    Eval {
        file: PathBuf,
        #[arg(long, value_enum, default_value = "search")]
        mode: EvalMode,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long, env = "JEV_MODEL", default_value = "jev-latest")]
        model: String,
        #[arg(long, env = "ULTRAFINANCE_MATCH_THRESHOLD", default_value = "0.95")]
        threshold: f64,
    },
    /// Maintain and search the local merchant database.
    Merchants {
        #[command(subcommand)]
        command: MerchantCommand,
    },
}

#[derive(Subcommand)]
enum MerchantCommand {
    /// Add a verified merchant, or replace one by supplying its existing ID.
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        country: Option<String>,
        #[arg(long)]
        website: Option<String>,
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
        /// Filter by a declared country; unknown countries are excluded.
        #[arg(long)]
        country: Option<String>,
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
    #[arg(long, conflicts_with_all = ["description", "amount", "currency", "date", "country", "extra"])]
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
    /// Additional evidence as a JSON object.
    #[arg(long)]
    extra: Option<String>,
    /// Use a JSON catalog instead of the SQLite database for this request.
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
    let cli = Cli::parse();
    match cli.command {
        Command::Eval {
            file,
            mode,
            output,
            model,
            threshold,
        } => {
            let contents = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            let mode = match mode {
                EvalMode::Search => ultrafinance_core::eval::Mode::Search,
                EvalMode::Enrich => ultrafinance_core::eval::Mode::Enrich,
            };
            let report = ultrafinance_core::eval::run(
                &contents,
                MerchantStore::open(&cli.database)?,
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
            eprintln!(
                "{} cases: retrieved expected merchant for {}/{} known cases; {} errors",
                report.metrics.cases,
                report.metrics.retrieval_hits,
                report.metrics.labeled_merchants,
                report.metrics.errors
            );
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
                let store = MerchantStore::memory()?;
                for merchant in load_catalog(Some(path))? {
                    store.put(&merchant)?;
                }
                store
            } else {
                MerchantStore::open(&cli.database)?
            };
            let enricher = Enricher::with_store(
                env::var("TYPESAFE_API_KEY").ok(),
                args.model,
                args.threshold,
                store,
            )?;
            let response = tokio::time::timeout(Duration::from_secs(25), enricher.enrich(&request))
                .await
                .context("merchant evaluation timed out")??;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        Command::Merchants { command } => {
            let store = MerchantStore::open(&cli.database)?;
            match command {
                MerchantCommand::Add {
                    name,
                    country,
                    website,
                    aliases,
                    sources,
                    id,
                } => {
                    let merchant = Merchant {
                        id: id.unwrap_or_else(|| format!("mer_{}", uuid::Uuid::new_v4().simple())),
                        name,
                        country,
                        website,
                        aliases,
                        sources,
                    };
                    store.put(&merchant)?;
                    println!("{}", serde_json::to_string_pretty(&merchant)?);
                }
                MerchantCommand::List {
                    country,
                    limit,
                    offset,
                    json,
                } => {
                    let page = store.list(country.as_deref(), limit, offset)?;
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
