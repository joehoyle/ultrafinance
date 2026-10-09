//! Token-authenticated OS Places export. Never surface engine output:
//! errors may contain SQL, bearer tokens or vended storage credentials.
use anyhow::{Context, Result, bail};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub fn export(executable: &Path, token: &str, region: &str, limit: Option<u32>) -> Result<tempfile::TempDir> {
    let dir = tempfile::tempdir()?;
    let output = dir.path().join("places.csv");
    let init = dir.path().join("empty-init");
    std::fs::write(&init, "")?;
    let filter = if region == "global" {
        "TRUE".into()
    } else {
        format!("upper(country) = {}", literal(&region.to_uppercase()))
    };
    let limit_clause = limit.map(|n| format!(" LIMIT {n}")).unwrap_or_default();
    let sql = format!(
        "INSTALL httpfs; LOAD httpfs; INSTALL iceberg; LOAD iceberg;\n\
         CREATE SECRET fsq_portal (TYPE iceberg, TOKEN {});\n\
         ATTACH 'places' AS fsq (TYPE iceberg, SECRET fsq_portal, ENDPOINT 'https://catalog.h3-hub.foursquare.com/iceberg', ACCESS_DELEGATION_MODE 'vended_credentials', READ_ONLY);\n\
         COPY (SELECT fsq_place_id, name, country, website, date_closed, date_refreshed, address, locality, region, postcode, latitude, longitude, to_json(fsq_category_ids) AS fsq_category_ids, to_json(fsq_category_labels) AS fsq_category_labels, to_json(unresolved_flags) AS unresolved_flags FROM fsq.datasets.places_os WHERE {filter} AND date_closed IS NULL ORDER BY fsq_place_id {limit_clause}) TO {} (FORMAT CSV, HEADER TRUE);\n",
        literal(token),
        literal(&output.to_string_lossy())
    );
    let mut child = Command::new(executable)
        .args(["-batch", "-bail", "-init"])
        .arg(init)
        .arg(":memory:")
        .env_remove("ULTRAFINANCE_FOURSQUARE_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Cannot start DuckDB; install the DuckDB CLI or set --duckdb-cli")?;
    let write = child
        .stdin
        .take()
        .context("DuckDB stdin unavailable")?
        .write_all(sql.as_bytes());
    if write.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        bail!(
            "Foursquare export failed while starting DuckDB (engine output suppressed to protect credentials)"
        );
    }
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                bail!(
                    "Foursquare export failed; check Places Portal token/access, network and DuckDB Iceberg support (engine output suppressed to protect credentials)"
                );
            }
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if !output.is_file() {bail!("Cannot read Foursquare export");}
    Ok(dir)
}
