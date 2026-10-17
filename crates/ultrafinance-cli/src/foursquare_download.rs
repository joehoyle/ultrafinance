//! Token-authenticated OS Places export. Never surface engine output:
//! errors may contain SQL, bearer tokens or vended storage credentials.
use anyhow::{Context, Result, bail};
use std::{
    fs::File,
    io::{IsTerminal, Read, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Default)]
struct CsvCount {
    quoted: bool,
    records: u64,
    bytes: u64,
}
impl CsvCount {
    fn scan(&mut self, bytes: &[u8]) {
        for byte in bytes {
            match byte {
                b'"' => self.quoted = !self.quoted,
                b'\n' if !self.quoted => self.records += 1,
                _ => {}
            }
        }
        self.bytes += bytes.len() as u64;
    }
    fn rows(&self) -> u64 {
        self.records.saturating_sub(1) // Exclude the header.
    }
}

struct DownloadProgress {
    started: Instant,
    last: Instant,
    terminal: bool,
}
impl DownloadProgress {
    fn new() -> Self {
        let now = Instant::now();
        let mut progress = Self {
            started: now,
            last: now,
            terminal: std::io::stderr().is_terminal(),
        };
        progress.render(&CsvCount::default(), None, "Preparing export", true);
        progress
    }
    fn render(&mut self, count: &CsvCount, total: Option<u64>, stage: &str, force: bool) {
        let interval = if self.terminal {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(5)
        };
        if !force && self.last.elapsed() < interval {
            return;
        }
        let rows = count.rows();
        let amount = total.map_or_else(
            || format!("{rows} rows"),
            |total| {
                let percent = if total == 0 {
                    if stage == "Export complete" {
                        100.0
                    } else {
                        0.0
                    }
                } else {
                    rows as f64 * 100.0 / total as f64
                };
                format!("{rows}/{total} rows ({:.1}%)", percent.min(100.0))
            },
        );
        let prefix = if self.terminal { "\r\x1b[2K" } else { "" };
        let end = if self.terminal { "" } else { "\n" };
        let mut stderr = std::io::stderr().lock();
        let _ = write!(
            stderr,
            "{prefix}Foursquare download: {amount} · {:.1} MiB · {}s · {stage}{end}",
            count.bytes as f64 / 1048576.0,
            self.started.elapsed().as_secs()
        );
        let _ = stderr.flush();
        self.last = Instant::now();
    }
}
impl Drop for DownloadProgress {
    fn drop(&mut self) {
        if self.terminal {
            let _ = writeln!(std::io::stderr());
        }
    }
}

fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn memory_limit() -> String {
    if let Ok(limit) = std::env::var("ULTRAFINANCE_DUCKDB_MEMORY_LIMIT") {
        return limit;
    }
    // DuckDB can see the host's RAM rather than Fargate's cgroup v1 limit.
    // Leave room for allocations outside its buffer manager and other processes.
    for path in [
        "/sys/fs/cgroup/memory.max",
        "/sys/fs/cgroup/memory/memory.limit_in_bytes",
    ] {
        if let Ok(contents) = std::fs::read_to_string(path)
            && let Ok(bytes) = contents.trim().parse::<u64>()
            && bytes > 0
            && bytes <= 1 << 40
        {
            return format!("{}B", bytes / 2);
        }
    }
    "4GiB".into()
}

pub fn export(
    executable: &Path,
    token: &str,
    region: &str,
    limit: Option<u32>,
) -> Result<tempfile::TempDir> {
    let dir = tempfile::tempdir()?;
    let output = dir.path().join("places.csv");
    let total_path = dir.path().join("total.csv");
    let init = dir.path().join("empty-init");
    std::fs::write(&init, "")?;
    let filter = if region == "global" {
        "TRUE".into()
    } else {
        format!("upper(country) = {}", literal(&region.to_uppercase()))
    };
    // Preparation already sorts and validates all place IDs using bounded runs.
    // Only capped downloads need ordering to choose a deterministic subset.
    let limit_clause = limit
        .map(|n| format!(" ORDER BY fsq_place_id LIMIT {n}"))
        .unwrap_or_default();
    let spill = dir.path().join("duckdb-spill");
    let count_expression =
        limit.map_or_else(|| "count(*)".into(), |n| format!("least(count(*), {n})"));
    let sql = format!(
        "SET memory_limit = {}; SET temp_directory = {}; SET preserve_insertion_order = false;\n\
         INSTALL httpfs; LOAD httpfs; INSTALL iceberg; LOAD iceberg;\n\
         CREATE SECRET fsq_portal (TYPE iceberg, TOKEN {});\n\
         ATTACH 'places' AS fsq (TYPE iceberg, SECRET fsq_portal, ENDPOINT 'https://catalog.h3-hub.foursquare.com/iceberg', ACCESS_DELEGATION_MODE 'vended_credentials', READ_ONLY);\n\
         COPY (SELECT {count_expression} FROM fsq.datasets.places_os WHERE {filter} AND date_closed IS NULL) TO {} (FORMAT CSV, HEADER FALSE);\n\
         COPY (SELECT fsq_place_id, name, country, website, date_closed, date_refreshed, address, locality, region, postcode, latitude, longitude, to_json(fsq_category_ids) AS fsq_category_ids, to_json(fsq_category_labels) AS fsq_category_labels, to_json(unresolved_flags) AS unresolved_flags FROM fsq.datasets.places_os WHERE {filter} AND date_closed IS NULL{limit_clause}) TO {} (FORMAT CSV, HEADER TRUE);\n",
        literal(&memory_limit()),
        literal(&spill.to_string_lossy()),
        literal(token),
        literal(&total_path.to_string_lossy()),
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
    let mut progress = DownloadProgress::new();
    let mut file = None;
    let mut count = CsvCount::default();
    let mut total = None;
    let mut buffer = [0; 65536];
    let result = (|| -> Result<()> {
        loop {
            if total.is_none()
                && let Ok(contents) = std::fs::read_to_string(&total_path)
                && contents.ends_with('\n')
            {
                total = contents.trim().parse::<u64>().ok();
            }
            if file.is_none() {
                file = File::open(&output).ok();
            }
            let status = child.try_wait()?;
            if let Some(status) = status
                && !status.success()
            {
                progress.render(&count, total, "Failed", true);
                bail!(
                    "Foursquare export failed ({status}); check memory limits, Places Portal token/access, network and DuckDB Iceberg support (engine output suppressed to protect credentials)"
                );
            }
            let mut caught_up = true;
            if let Some(file) = &mut file {
                for _ in 0..128 {
                    let size = file.read(&mut buffer)?;
                    if size == 0 {
                        break;
                    }
                    count.scan(&buffer[..size]);
                    caught_up = false;
                }
            }
            if status.is_some() && caught_up {
                if !output.is_file() {
                    bail!("Cannot read Foursquare export");
                }
                progress.render(&count, total, "Export complete", true);
                break;
            }
            progress.render(
                &count,
                total,
                if status.is_some() {
                    "Checking CSV"
                } else if file.is_some() {
                    "Downloading"
                } else {
                    "Preparing export"
                },
                false,
            );
            if status.is_none() {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counts_csv_records_across_partial_writes_and_quoted_newlines() {
        let csv = b"id,name\r\na,\"first\nline\"\r\nb,\"a \"\"quoted\"\" name\"\nc,last\n";
        for size in 1..=csv.len() {
            let mut count = CsvCount::default();
            for chunk in csv.chunks(size) {
                count.scan(chunk);
            }
            assert_eq!(count.rows(), 3);
            assert_eq!(count.bytes, csv.len() as u64);
            assert!(!count.quoted);
        }
        let mut count = CsvCount::default();
        count.scan(b"id,name\na,\"unfinished\n");
        assert_eq!(count.rows(), 0);
        count.scan(b"field\"\n");
        assert_eq!(count.rows(), 1);
    }
}
