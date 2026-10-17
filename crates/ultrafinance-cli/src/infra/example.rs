use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{io::Write, path::Path, process::Command};

/// This is a deployment operation, never a homepage request operation.
pub fn refresh(root: &Path, site: &str) -> Result<()> {
    let path = root.join("website/example.json");
    let saved: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    let request = &saved["request"];
    serde_json::from_value::<ultrafinance_core::EnrichRequest>(request.clone())?.validate()?;
    let site = reqwest::Url::parse(site)?;
    if site.scheme() != "https" || !site.username().is_empty() || site.password().is_some() {
        bail!("homepage example source must be an HTTPS URL without credentials");
    }
    let endpoint = site.join("/v1/enrich")?;
    // Resume a sleeping database with a read before the single POST capture.
    // curl bounds each attempt and retries transient HTTP errors twice.
    let readiness = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--max-time",
            "70",
            "--retry",
            "2",
            "--retry-delay",
            "2",
            site.join("/v1/merchants?limit=1")?.as_str(),
        ])
        .output()
        .context("could not run curl for homepage catalog readiness")?;
    if !readiness.status.success() {
        bail!("homepage catalog readiness failed ({})", readiness.status);
    }
    eprintln!("Capturing homepage example from {endpoint}");
    let output = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--max-time",
            "70",
            "--header",
            "Content-Type: application/json",
            "--data-raw",
            &serde_json::to_string(request)?,
            endpoint.as_str(),
        ])
        .output()
        .context("could not run curl to capture homepage example")?;
    if !output.status.success() {
        bail!("homepage example capture failed ({})", output.status);
    }
    let response: Value = serde_json::from_slice(&output.stdout)
        .context("homepage example API returned invalid JSON")?;
    let captured = capture(
        saved,
        response,
        endpoint.as_str(),
        &crate::build_metadata::BuildMetadata::capture(root).time,
    )?;
    // Keep the previous snapshot intact if fetching or validation fails.
    let mut temporary = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    serde_json::to_writer_pretty(&mut temporary, &captured)?;
    temporary.write_all(b"\n")?;
    temporary.persist(&path)?;
    eprintln!(
        "Saved homepage example captured at {}",
        captured["captured_at"]
    );
    Ok(())
}

fn capture(mut saved: Value, response: Value, endpoint: &str, time: &str) -> Result<Value> {
    verify_response(&saved, &response)?;
    saved["response"] = response;
    saved["endpoint"] = endpoint.into();
    saved["captured_at"] = time.into();
    Ok(saved)
}

pub fn verify_response(saved: &Value, response: &Value) -> Result<()> {
    let expected = saved["response"]["merchant"]["data"]["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .context("saved homepage example must identify the expected merchant")?;
    if response["merchant"]["status"] != "matched"
        || response["merchant"]["data"]["id"] != expected
        || response["merchant"]["data"]["name"]
            .as_str()
            .is_none_or(str::is_empty)
    {
        bail!(
            "homepage example no longer matches the expected merchant; review the request before releasing"
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/infra/example/tests.rs"]
mod tests;
