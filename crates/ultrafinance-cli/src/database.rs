use anyhow::{Context, Result, bail};
use std::process::Command;

pub(crate) fn open(connection: &str) -> Result<()> {
    let mut url = reqwest::Url::parse(connection)
        .map_err(|_| anyhow::anyhow!("database open requires a PostgreSQL connection URL"))?;
    if !matches!(url.scheme(), "postgres" | "postgresql")
        || url.host_str().is_none_or(str::is_empty)
        || url.fragment().is_some()
    {
        bail!("database open requires a PostgreSQL connection URL with a host");
    }
    url.set_scheme("postgres")
        .map_err(|_| anyhow::anyhow!("could not prepare PostgreSQL connection URL"))?;

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg("--");
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = Command::new("xdg-open");

    // Avoid echoing credentials from the URL or from the handler's diagnostics.
    let output = command
        .arg(url.as_str())
        .output()
        .context("could not launch the default PostgreSQL URL handler")?;
    if !output.status.success() {
        bail!("could not open database; configure an application to handle postgres:// URLs");
    }
    println!("Opened database in the default PostgreSQL client");
    Ok(())
}
