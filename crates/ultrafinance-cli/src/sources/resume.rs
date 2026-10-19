//! Interactive selection of interrupted Foursquare imports.
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    fmt,
    io::IsTerminal,
    path::{Path, PathBuf},
};

struct Candidate {
    path: PathBuf,
    processed: u64,
    total: u64,
}

fn discover(output: &Path, region: &str, merchants_only: bool) -> Result<Vec<Candidate>> {
    let directory = output.join("foursquare");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("cannot list Foursquare checkpoints"),
    };
    let mut candidates = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        let checkpoint = path.join("checkpoint.json");
        if !checkpoint.is_file() {
            continue;
        }
        let value: Value = match serde_json::from_reader(std::fs::File::open(&checkpoint)?) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let apply = &value["application"];
        if value["format"].as_u64() != Some(1)
            || value["stage"].as_str() != Some("Preparing")
            || value["manifest"]["source"].as_str() != Some("foursquare")
            || value["manifest"]["region"].as_str() != Some(region)
            || apply["complete"].as_bool() != Some(false)
            || apply["merchants_only"].as_bool().unwrap_or(false) != merchants_only
        {
            continue;
        }
        candidates.push(Candidate {
            path,
            processed: apply["processed"].as_u64().unwrap_or(0),
            total: value["total_rows"].as_u64().unwrap_or(0),
        });
    }
    candidates.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(candidates)
}

impl fmt::Display for Candidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} — {}/{} rows committed",
            self.path.display(),
            self.processed,
            self.total
        )
    }
}

pub(super) fn choose(output: &Path, region: &str, merchants_only: bool) -> Result<PathBuf> {
    let candidates = discover(output, region, merchants_only)?;
    if candidates.is_empty() {
        bail!(
            "no interrupted Foursquare checkpoints for region {region} and this import mode under {}; use --output to search elsewhere or --resume PATH",
            output.join("foursquare").display()
        );
    }
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        bail!("checkpoint selection requires a terminal; pass --resume PATH explicitly");
    }
    let selected = inquire::Select::new("Choose an interrupted Foursquare import:", candidates)
        .with_help_message("↑/↓ to move, type to filter, Enter to resume, Esc to cancel")
        .prompt()
        .map_err(|error| match error {
            inquire::InquireError::OperationCanceled
            | inquire::InquireError::OperationInterrupted => {
                anyhow::anyhow!("checkpoint selection cancelled")
            }
            error => anyhow::anyhow!(error).context("cannot select resume checkpoint"),
        })?;
    Ok(selected.path)
}

#[cfg(test)]
#[path = "../../tests/unit/sources/resume_tests.rs"]
mod tests;
