//! Administrative website image discovery. No API request performs this work.
mod discover;
mod images;
mod storage;
use anyhow::{Context, Result, bail};
use clap::{Args as ClapArgs, Subcommand};
use std::path::PathBuf;
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
    /// Inspect a saved logo, including original and display asset metadata.
    Get {
        website: String,
        #[arg(long)]
        json: bool,
    },
    /// Discover and store logos for website URLs (repeatable, sequential batch).
    Discover {
        #[arg(required = true)]
        websites: Vec<String>,
        /// Recheck successful associations; otherwise reuse them indefinitely.
        #[arg(long)]
        refresh: bool,
        /// Explicit reviewed image URL, for one website only.
        #[arg(long)]
        image_url: Option<String>,
        /// Asset root directory. Serve this root at the public base URL.
        #[arg(long, conflicts_with = "bucket")]
        directory: Option<PathBuf>,
        /// S3 bucket for immutable assets (uses standard AWS CLI credentials).
        #[arg(long, env = "ULTRAFINANCE_LOGO_BUCKET", conflicts_with = "directory")]
        bucket: Option<String>,
        /// HTTPS origin, e.g. https://ultrafinance.app; keys use /logos/.
        #[arg(long, env = "ULTRAFINANCE_LOGO_BASE_URL")]
        public_base_url: String,
    },
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
            websites,
            refresh,
            image_url,
            directory,
            bucket,
            public_base_url,
        } => {
            if image_url.is_some() && websites.len() != 1 {
                bail!("--image-url requires exactly one website");
            }
            let storage = storage::Storage::new(directory, bucket, &public_base_url)?;
            let mut failures = 0;
            for website in websites {
                let result: Result<WebsiteLogo> = async {
                    let website = normalize_website(&website)?;
                    if !refresh && let Some(logo) = store.website_logo(&website)? {
                        return Ok(logo);
                    }
                    let discovered = discover::discover(&website, image_url.as_deref()).await?;
                    let original = storage.publish(
                        &discovered.image.original,
                        discovered.image.mime,
                        discovered.image.extension,
                        discovered.image.width,
                        discovered.image.height,
                        true,
                    )?;
                    let display = storage.publish(
                        &discovered.image.display,
                        "image/webp",
                        "webp",
                        discovered.image.display_width,
                        discovered.image.display_height,
                        false,
                    )?;
                    let logo = WebsiteLogo {
                        website_url: website.clone(),
                        source_url: discovered.source_url,
                        kind: discovered.kind,
                        original,
                        display,
                        checked_at: None,
                    };
                    // Upload both assets before publishing the association. Failures
                    // leave only harmless unreferenced immutable files.
                    store
                        .save_website_logo(&logo)
                        .context("cannot save logo; run `ultrafinance database init` first")?;
                    store.website_logo(&website)?.context("saved logo missing")
                }
                .await;
                match result {
                    Ok(logo) => println!("{}", serde_json::to_string(&logo)?),
                    Err(error) => {
                        failures += 1;
                        eprintln!("Logo discovery failed for {website}: {error:#}");
                    }
                }
            }
            if failures > 0 {
                bail!(
                    "{failures} website(s) failed logo discovery; successful associations were saved"
                );
            }
        }
    }
    Ok(())
}
