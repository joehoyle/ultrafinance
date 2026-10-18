//! CLI presentation for the shared research engine.
use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use std::{path::PathBuf, time::Duration};
use ultrafinance_core::{EnrichRequest, research::Research, store::MerchantStore};
#[derive(ClapArgs)]
pub struct Args {
    pub description: String,
    #[arg(long)]
    pub country: Option<String>,
    #[arg(
        long,
        env = "ULTRAFINANCE_RESEARCH_MODEL",
        default_value = "gpt-6.1-sol"
    )]
    pub model: String,
    /// Save a supported new business to the catalog, without learning a descriptor alias.
    #[arg(long)]
    pub create: bool,
    /// Print the typed result as compact JSON (default: pretty JSON).
    #[arg(long)]
    pub json: bool,
    /// Trace LLM request/response bodies and local tool calls/results to stderr.
    #[arg(long)]
    pub details: bool,
    /// Also save the complete JSON report to a file.
    #[arg(long)]
    pub output: Option<PathBuf>,
}

pub async fn run(args: Args, database_url: Option<&str>) -> Result<()> {
    let request: EnrichRequest = serde_json::from_value(
        serde_json::json!({"description":args.description,"country":args.country}),
    )?;
    request.validate()?;
    let research = Research::from_env()?
        .context("research requires OPENAI_API_KEY")?
        .with_model(args.model)?;
    let store = MerchantStore::configured(database_url)?;
    let report = research
        .investigate(
            &request,
            &store,
            args.create,
            args.details,
            Duration::from_secs(300),
        )
        .await?;
    if let Some(path) = &args.output {
        std::fs::write(path, serde_json::to_string_pretty(&report)?)
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    let result = ultrafinance_core::research::result_json(&report)?;
    println!(
        "{}",
        if args.json {
            serde_json::to_string(&result)?
        } else {
            serde_json::to_string_pretty(&result)?
        }
    );
    Ok(())
}
