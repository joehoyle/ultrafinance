//! Administrative website image discovery. No API request performs this work.
mod discover;
mod images;
mod progress;
mod storage;
mod workers;
use anyhow::{Context, Result, bail};
use clap::{Args as ClapArgs, Subcommand};
use std::{collections::HashSet, path::PathBuf, sync::Arc};
use ultrafinance_core::{
    logos::{WebsiteLogo, normalize_website},
    store::MerchantStore,
};

#[derive(ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// List saved website logos without fetching any images.
    List {
        #[arg(long, default_value = "50", value_parser = clap::value_parser!(u32).range(1..=1000))]
        limit: u32,
        #[arg(long, default_value = "0")]
        offset: usize,
        #[arg(long)]
        json: bool,
    },
    /// Inspect a saved logo, including display asset metadata.
    Get {
        website: String,
        #[arg(long)]
        json: bool,
    },
    /// Discover logos for website URLs or all eligible websites in a market.
    Discover {
        #[arg(required_unless_present = "market", conflicts_with = "market")]
        websites: Vec<String>,
        /// Select merchants operating in a country, e.g. CA (not website country/TLD).
        #[arg(long, value_parser = parse_market)]
        market: Option<String>,
        /// Process at most this many unique websites in market mode.
        #[arg(long, requires = "market", value_parser = clap::value_parser!(u32).range(1..))]
        limit: Option<u32>,
        /// Maximum concurrent website jobs (network I/O, not OS threads).
        #[arg(long, alias = "threads", default_value = "4", value_parser = clap::value_parser!(u32).range(1..=64))]
        workers: u32,
        /// Show how many websites remain, without fetching images or changing data.
        #[arg(long)]
        dry_run: bool,
        /// Recheck successful associations; otherwise reuse them indefinitely.
        #[arg(long)]
        refresh: bool,
        /// Explicit reviewed image URL, for one website only.
        #[arg(long, conflicts_with = "market")]
        image_url: Option<String>,
        /// Local logo directory (defaults to ./logos when no bucket is configured).
        #[arg(long, conflicts_with = "bucket")]
        directory: Option<PathBuf>,
        /// S3 bucket for immutable assets (uses standard AWS CLI credentials).
        #[arg(long, env = "ULTRAFINANCE_LOGO_BUCKET", conflicts_with = "directory")]
        bucket: Option<String>,
    },
}
fn parse_market(value: &str) -> std::result::Result<String, String> {
    if value.len() != 2 || !value.bytes().all(|c| c.is_ascii_alphabetic()) {
        return Err("market must be a two-letter country code, e.g. CA".into());
    }
    Ok(value.to_ascii_uppercase())
}

pub async fn run(args: Args, database_url: Option<&str>) -> Result<()> {
    // Construct the synchronous worker off the Tokio runtime.
    let database_url = database_url.map(str::to_owned);
    let store =
        tokio::task::spawn_blocking(move || MerchantStore::configured(database_url.as_deref()))
            .await??;
    match args.command {
        Command::List {
            limit,
            offset,
            json,
        } => {
            crate::output::logo_list(&store.list_website_logos(limit as usize, offset)?, json)?;
        }
        Command::Get { website, json } => {
            crate::output::logo_detail(store.website_logo(&website)?.as_ref(), &website, json)?;
        }
        Command::Discover {
            mut websites,
            market,
            limit,
            dry_run,
            workers,
            refresh,
            image_url,
            directory,
            bucket,
        } => {
            if image_url.is_some() && websites.len() != 1 {
                bail!("--image-url requires exactly one website");
            }
            let market_mode = market.is_some();
            let market_progress = market
                .as_ref()
                .filter(|_| !dry_run)
                .map(|market| progress::Progress::new(market.clone()));
            if let Some(market) = market {
                let selection_store = store.clone();
                if dry_run {
                    eprintln!("Selecting unique websites for market {market}...");
                }
                websites = tokio::task::spawn_blocking(move || {
                    selection_store.logo_discovery_websites(
                        &market,
                        refresh,
                        if dry_run {
                            None
                        } else {
                            limit.map(|v| v as usize)
                        },
                    )
                })
                .await??;
            }
            // Different spellings of the same website must not race discovery.
            let mut seen = HashSet::new();
            websites.retain(|website| {
                seen.insert(normalize_website(website).unwrap_or_else(|_| website.clone()))
            });
            if dry_run {
                if !market_mode && !refresh {
                    let lookup_store = store.clone();
                    websites = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
                        let mut pending = Vec::new();
                        for website in websites {
                            if lookup_store.website_logo(&website)?.is_none() {
                                pending.push(website);
                            }
                        }
                        Ok(pending)
                    })
                    .await??;
                }
                let remaining = websites.len();
                let selected = limit.map_or(remaining, |limit| remaining.min(limit as usize));
                if refresh {
                    println!("Websites eligible for refresh: {remaining}");
                } else {
                    println!("Remaining websites: {remaining}");
                }
                println!("Selected for this run: {selected}");
                return Ok(());
            }
            let total = websites.len();
            if market_progress.is_none() {
                eprintln!("Selected {total} website(s)");
            }
            let storage = Arc::new(storage::Storage::new(directory, bucket)?);
            let mut failures = 0;
            let mut completed = 0;
            if let Some(progress) = &market_progress {
                progress.discover(total, workers);
            } else {
                eprintln!("Discovering with {workers} worker(s)");
            }
            workers::run(
                websites,
                workers as usize,
                |website| {
                    let store = store.clone();
                    let storage = storage.clone();
                    let image_url = image_url.clone();
                    async move {
                        let result =
                            discover_one(store, storage, &website, refresh, image_url).await;
                        (website, result)
                    }
                },
                |(website, result)| {
                    completed += 1;
                    if let Some(progress) = &market_progress {
                        progress.record(result.is_ok());
                        if result.is_err() {
                            failures += 1;
                        }
                        return Ok(());
                    }
                    eprintln!("[{completed}/{total}] {website}");
                    match result {
                        Ok(logo) => println!("{}", crate::result_json::compact(&logo)?),
                        Err(error) => {
                            failures += 1;
                            eprintln!("Logo discovery failed for {website}: {error:#}");
                        }
                    }
                    Ok(())
                },
            )
            .await?;
            drop(market_progress);
            if failures > 0 {
                bail!(
                    "{failures} website(s) failed logo discovery; successful associations were saved"
                );
            }
        }
    }
    Ok(())
}

async fn discover_one(
    store: MerchantStore,
    storage: Arc<storage::Storage>,
    website: &str,
    refresh: bool,
    image_url: Option<String>,
) -> Result<WebsiteLogo> {
    let website = normalize_website(website)?;
    if !refresh {
        let lookup_store = store.clone();
        let lookup_website = website.clone();
        if let Some(logo) =
            tokio::task::spawn_blocking(move || lookup_store.website_logo(&lookup_website))
                .await??
        {
            return Ok(logo);
        }
    }
    let discovered = discover::discover(&website, image_url.as_deref()).await?;
    // Filesystem/S3 publishing and synchronous database calls must not block
    // Tokio's network workers. A job retains its slot until the upload and
    // the association save finish, bounding CPU work and uploads as well.
    tokio::task::spawn_blocking(move || {
        let image = discovered.image;
        let display = storage.publish(
            &image.display,
            "image/webp",
            "webp",
            image.display_width,
            image.display_height,
        )?;
        let logo = WebsiteLogo {
            website_url: website.clone(),
            source_url: discovered.source_url,
            kind: discovered.kind,
            display,
            checked_at: None,
        };
        store
            .save_website_logo(&logo)
            .context("cannot save logo; run `ultrafinance database init` first")?;
        store.website_logo(&website)?.context("saved logo missing")
    })
    .await?
}
