use anyhow::{Context, Result, bail};
use clap::{Args as ClapArgs, Subcommand};
use serde_json::Value;
use std::{io::Read, path::PathBuf};
use ultrafinance_core::{
    CounterpartyType, processor_formats::ProcessorFormat, store::MerchantStore,
};

#[derive(ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List stored descriptor formats as JSON.
    List,
    /// Test stored formats against a descriptor without enrichment or provider calls.
    Match { description: String },
    /// Import researched JSON; each entry references a processor by exact catalog name.
    Import {
        file: PathBuf,
        /// Validate formats and processor identities without writing changes.
        #[arg(long)]
        dry_run: bool,
    },
}

pub fn run(args: Args, database_url: Option<&str>) -> Result<()> {
    let store = MerchantStore::configured(database_url)?;
    match args.command {
        Command::Match { description } => {
            if description.trim().is_empty() || description.len() > 4096 {
                bail!("descriptor must be nonblank and at most 4096 bytes");
            }
            let mut matches = store.processor_format_matches(&[description])?;
            println!("{}", crate::result_json::pretty(&matches.remove(0))?);
        }
        Command::List => println!(
            "{}",
            crate::result_json::pretty(&store.processor_formats()?)?
        ),
        Command::Import { file, dry_run } => {
            let mut contents = Vec::new();
            std::fs::File::open(file)?
                .take(2 * 1024 * 1024 + 1)
                .read_to_end(&mut contents)?;
            if contents.len() > 2 * 1024 * 1024 {
                bail!("processor format import exceeds 2 MiB");
            }
            let records: Vec<Value> = serde_json::from_slice(&contents)?;
            if records.is_empty() || records.len() > 1000 {
                bail!("import requires 1–1000 processor formats");
            }
            let mut processors = Vec::new();
            let mut offset = 0;
            loop {
                let page = store.list_typed(
                    None,
                    1000,
                    offset,
                    Some(CounterpartyType::PaymentProcessor),
                )?;
                let count = page.merchants.len();
                processors.extend(page.merchants);
                offset += count;
                if offset >= page.total || count == 0 {
                    break;
                }
            }
            let mut formats = Vec::new();
            for mut record in records {
                let name = record
                    .as_object_mut()
                    .context("processor format must be an object")?
                    .remove("processor")
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .context("processor format requires processor catalog name")?;
                if record.get("processor_id").is_some() {
                    bail!("use processor name, not processor_id, in format imports");
                }
                let matched: Vec<_> = processors
                    .iter()
                    .filter(|processor| processor.name.eq_ignore_ascii_case(&name))
                    .collect();
                if matched.len() != 1 {
                    bail!(
                        "processor name {name:?} must identify exactly one existing payment_processor"
                    );
                }
                record["processor_id"] = serde_json::json!(matched[0].id);
                formats.push(serde_json::from_value::<ProcessorFormat>(record)?);
            }
            let count = formats.len();
            store.save_processor_formats(formats, dry_run)?;
            println!(
                "{count} processor formats {}",
                if dry_run {
                    "validated; no changes written"
                } else {
                    "imported"
                }
            );
        }
    }
    Ok(())
}
