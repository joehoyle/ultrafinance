use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{path::Path, process::Command};

pub trait Api {
    fn call(&self, args: &[&str]) -> Result<Value>;
}

pub struct Aws {
    pub profile: String,
    pub region: String,
}

impl Aws {
    pub fn configured(root: &Path) -> Result<Self> {
        Ok(Self {
            profile: super::output(root, "aws_profile")?,
            region: super::output(root, "aws_region")?,
        })
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new("aws");
        if !self.profile.is_empty() {
            command.args(["--profile", &self.profile]);
        }
        command
            .args(["--region", &self.region])
            .env("AWS_PAGER", "")
            .env("AWS_REGION", &self.region)
            .env("AWS_DEFAULT_REGION", &self.region);
        command
    }
}

impl Api for Aws {
    fn call(&self, args: &[&str]) -> Result<Value> {
        let result = self
            .command()
            .args(args)
            .args(["--output", "json"])
            .output()
            .context("could not run AWS CLI")?;
        if !result.status.success() {
            let diagnostic = String::from_utf8_lossy(&result.stderr);
            let lower = diagnostic.to_lowercase();
            if ["expired", "sso", "login", "credential"]
                .iter()
                .any(|s| lower.contains(s))
            {
                bail!(
                    "AWS authentication failed; log in to profile {} and retry",
                    self.profile
                );
            }
            // Print only the AWS error code, never responses or sensitive arguments.
            let code = diagnostic
                .split("An error occurred (")
                .nth(1)
                .and_then(|s| s.split(')').next())
                .unwrap_or("AWS CLI failed");
            bail!(
                "{} {}: {code}",
                args.first().unwrap_or(&"aws"),
                args.get(1).unwrap_or(&"")
            );
        }
        if result.stdout.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&result.stdout).context("invalid JSON from AWS CLI")
    }
}

pub fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("AWS response missing {key}"))
}

pub fn validate_image(image: &str) -> Result<()> {
    let pattern = regex::Regex::new(
        r"^[0-9]{12}\.dkr\.ecr\.[a-z0-9-]+\.amazonaws\.com/[a-z0-9/_-]+@sha256:[a-f0-9]{64}$",
    )?;
    if !pattern.is_match(image) {
        bail!("use an immutable ECR sha256 image digest, not a tag");
    }
    Ok(())
}
