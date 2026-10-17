use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
mod aws;
mod cleanup;
mod example;
mod import_db;
mod persistent;
mod release;
mod sessions;
mod shell;
mod terminal;

#[derive(Args)]
pub struct InfraArgs {
    /// Workspace root; otherwise search the current directory and its parents.
    #[arg(long, global = true)]
    workspace: Option<PathBuf>,
    #[command(subcommand)]
    command: InfraCommand,
}

#[derive(Subcommand)]
enum InfraCommand {
    /// Build an ARM64 image, check a published version, and promote live.
    Deploy {
        /// Release an already-pushed immutable ECR digest without rebuilding.
        #[arg(long)]
        image: Option<String>,
    },
    /// Capture a real API response for the static homepage example.
    RefreshExample {
        #[arg(long, default_value = "https://ultrafinance.app")]
        site_url: String,
    },
    /// Open the interactive production CLI shell using ECS Exec.
    #[command(alias = "shell")]
    Cli {
        /// Immutable ECR digest; defaults to Lambda's live image.
        #[arg(long, conflicts_with = "latest")]
        image: Option<String>,
        /// Use the newest published Lambda version, even if it has not been promoted to live.
        #[arg(long)]
        latest: bool,
        /// Keep the task and tmux session running after disconnecting.
        #[arg(long)]
        persistent: bool,
        /// Persistent task lifetime from startup, in hours (default: 24).
        #[arg(long, requires = "persistent", value_parser = clap::value_parser!(u32).range(1..=168))]
        lifetime_hours: Option<u32>,
        /// Restore scratch files from a completed EBS snapshot in this AWS region.
        #[arg(long)]
        snapshot: Option<String>,
    },
    /// Reconnect to an existing persistent CLI task and its tmux session.
    CliAttach { session_id: String },
    /// List active production CLI session IDs, statuses, and start times.
    CliList,
    /// Stop one persistent CLI task, optionally saving its scratch data first.
    CliStop {
        session_id: String,
        /// Wait for a scratch disk snapshot to complete before stopping.
        #[arg(long)]
        save_data: bool,
        /// Reuse an existing snapshot of this session's scratch disk.
        #[arg(long, requires = "save_data")]
        snapshot: Option<String>,
        /// Maximum snapshot wait in hours (default: 6); does not extend task lifetime.
        #[arg(long, requires = "save_data", value_parser = clap::value_parser!(u32).range(1..=24))]
        wait_hours: Option<u32>,
    },
    /// Copy the full production database into the default local PostgreSQL database.
    ImportDb,
    /// Stop all production CLI tasks, including any active shell sessions.
    CliCleanup,
    /// Read Lambda application logs from CloudWatch (not enrichment history).
    Logs {
        /// How far back to read, using AWS CLI duration syntax.
        #[arg(long, default_value = "10m")]
        since: String,
        /// Stream new log events until interrupted.
        #[arg(long)]
        follow: bool,
    },
}

fn is_workspace(path: &Path) -> bool {
    path.join("Cargo.toml").is_file() && path.join("infra/outputs.tf").is_file()
}

fn workspace(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        let path = path
            .canonicalize()
            .context("cannot open workspace directory")?;
        if !is_workspace(&path) {
            bail!("workspace must contain Cargo.toml and infra/outputs.tf");
        }
        return Ok(path);
    }
    let current = std::env::current_dir()?;
    current
        .ancestors()
        .find(|path| is_workspace(path))
        .map(Path::to_path_buf)
        .context("run inside the Ultrafinance workspace or pass --workspace PATH")
}

fn checked(command: &mut Command) -> Result<()> {
    let program = command.get_program().to_string_lossy().into_owned();
    let status = command
        .status()
        .with_context(|| format!("could not run {program}"))?;
    if !status.success() {
        bail!("{program} failed ({status})");
    }
    Ok(())
}

fn output(root: &Path, name: &str) -> Result<String> {
    let variable = match name {
        "aws_profile" => Some("AWS_PROFILE"),
        "aws_region" => Some("AWS_REGION"),
        "function_name" => Some("LAMBDA_FUNCTION_NAME"),
        "repository_url" => Some("ECR_REPOSITORY"),
        "site_url" => Some("ULTRAFINANCE_SITE_URL"),
        _ => None,
    };
    if let Some(variable) = variable
        && let Ok(value) = std::env::var(variable)
    {
        if value.is_empty() && name != "aws_profile" {
            bail!("{variable} must not be empty");
        }
        return Ok(value);
    }
    let result = Command::new("tofu")
        .current_dir(root)
        .args(["-chdir=infra", "output", "-raw", name])
        .output()
        .context("could not run tofu; install OpenTofu first")?;
    // Do not print state or configuration diagnostics.
    if !result.status.success() {
        bail!(
            "could not read OpenTofu output {name}; initialize the existing infrastructure first"
        );
    }
    let value = String::from_utf8(result.stdout)?.trim().to_owned();
    if value.is_empty() || value == "null" {
        bail!("OpenTofu output {name} is not configured");
    }
    Ok(value)
}

pub fn run(args: InfraArgs) -> Result<()> {
    let root = workspace(args.workspace)?;
    match args.command {
        InfraCommand::Deploy { image } => release::deploy(&root, image.as_deref()),
        InfraCommand::RefreshExample { site_url } => example::refresh(&root, &site_url),
        InfraCommand::Cli {
            image,
            latest,
            persistent,
            lifetime_hours,
            snapshot,
        } => {
            let selection = if latest {
                shell::ImageSelection::Latest
            } else {
                image
                    .as_deref()
                    .map_or(shell::ImageSelection::Live, shell::ImageSelection::Digest)
            };
            let mode = if persistent {
                shell::SessionMode::Persistent {
                    hours: lifetime_hours.unwrap_or(24),
                }
            } else {
                shell::SessionMode::Temporary
            };
            shell::open(&root, selection, mode, snapshot.as_deref())
        }
        InfraCommand::CliAttach { session_id } => persistent::attach(&root, &session_id),
        InfraCommand::CliList => sessions::run(&root),
        InfraCommand::CliStop {
            session_id,
            save_data,
            snapshot,
            wait_hours,
        } => persistent::stop(
            &root,
            &session_id,
            save_data,
            snapshot.as_deref(),
            wait_hours.unwrap_or(6),
        ),
        InfraCommand::ImportDb => import_db::run(&root),
        InfraCommand::CliCleanup => cleanup::run(&root),
        InfraCommand::Logs { since, follow } => {
            let profile = output(&root, "aws_profile")?;
            let region = output(&root, "aws_region")?;
            let function = output(&root, "function_name")?;
            let mut command = Command::new("aws");
            command
                .current_dir(&root)
                .env("AWS_PAGER", "")
                .args(["--profile", &profile, "--region", &region, "logs", "tail"])
                .arg(format!("/aws/lambda/{function}"))
                .args(["--since", &since, "--format", "short"]);
            if follow {
                command.arg("--follow");
            }
            checked(&mut command)
        }
    }
}
