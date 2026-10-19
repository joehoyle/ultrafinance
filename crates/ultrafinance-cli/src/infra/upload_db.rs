//! Local -> Aurora logical copy. S3 carries archives; ECS Exec carries only control.
use super::{
    aws::{Api, Aws, text},
    dump_progress::DumpProgress,
    import_db::{checked, local_command, spawn},
    progress::{Counter, Progress, Unit, uploaded_bytes},
    shell::{self, ImageSelection, Interrupts},
};
use anyhow::{Context, Result, bail};
use clap::Args;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use ultrafinance_core::store::POSTGRES_SCHEMA_VERSION;

#[derive(Args)]
pub(super) struct Options {
    /// Override the managed private S3 transfer bucket (in the configured AWS region).
    #[arg(long)]
    bucket: Option<String>,
    /// Immutable ECR image containing PostgreSQL 17 clients; defaults to Lambda's live image.
    #[arg(long, conflicts_with = "latest")]
    image: Option<String>,
    /// Use the newest published Lambda image, even before promotion to live.
    #[arg(long)]
    latest: bool,
    /// Reuse a completed S3 transfer, skipping the local database dump and upload.
    #[arg(long, conflicts_with_all = ["dump_jobs", "postgres_bin"])]
    resume: Option<String>,
    /// Validate a saved S3 archive without launching a task or changing production.
    #[arg(long, requires = "resume")]
    check_only: bool,
    /// Directory containing PostgreSQL 17 pg_dump and psql; otherwise auto-detected.
    #[arg(long)]
    postgres_bin: Option<PathBuf>,
    /// Parallel local dump workers; defaults to the local CPU count.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=128))]
    dump_jobs: Option<u32>,
    /// Parallel restore workers; defaults to twice the task CPU count (at most 32),
    /// further limited by available database connection headroom.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=128))]
    restore_jobs: Option<u32>,
    /// Concurrent archive uploads and downloads.
    #[arg(long, default_value_t = 16, value_parser = clap::value_parser!(u32).range(1..=128))]
    transfer_jobs: u32,
}

impl Options {
    fn image_selection(&self) -> ImageSelection<'_> {
        if self.latest {
            ImageSelection::Latest
        } else {
            self.image
                .as_deref()
                .map_or(ImageSelection::Live, ImageSelection::Digest)
        }
    }
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn major(program: &Path) -> Result<u32> {
    let result = Command::new(program)
        .arg("--version")
        .output()
        .with_context(|| format!("could not run PostgreSQL client {}", program.display()))?;
    if result.status.success()
        && let Some(major) = String::from_utf8_lossy(&result.stdout)
            .split_whitespace()
            .find_map(|part| part.split('.').next()?.parse().ok())
    {
        return Ok(major);
    }
    bail!("could not identify {} version", program.display())
}

#[derive(Debug)]
struct PgTools {
    directory: PathBuf,
}

impl PgTools {
    fn find(directories: impl IntoIterator<Item = PathBuf>) -> Option<Self> {
        directories.into_iter().find_map(|directory| {
            // Use a matching pair. A newer dump archive may not be readable by
            // the version 17 restore client in the AWS image.
            let matches = ["pg_dump", "psql"].iter().all(|name| {
                let program = directory.join(name);
                program.is_file() && major(&program).is_ok_and(|major| major == 17)
            });
            matches.then_some(Self { directory })
        })
    }

    fn resolve(explicit: Option<&Path>) -> Result<Self> {
        if let Some(directory) = explicit {
            return Self::find([directory.to_path_buf()]).with_context(|| {
                format!(
                    "--postgres-bin {} must contain PostgreSQL 17 pg_dump and psql",
                    directory.display()
                )
            });
        }
        let mut directories: Vec<_> =
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
        // Homebrew versioned clients are keg-only, so they are commonly absent
        // from PATH. Debian/Ubuntu also keep major versions in separate bins.
        directories.extend(
            [
                "/opt/homebrew/opt/libpq@17/bin",
                "/opt/homebrew/opt/postgresql@17/bin",
                "/usr/local/opt/libpq@17/bin",
                "/usr/local/opt/postgresql@17/bin",
                "/usr/lib/postgresql/17/bin",
            ]
            .map(PathBuf::from),
        );
        Self::find(directories).context("infra upload-db needs PostgreSQL 17 clients. On macOS run `brew install libpq@17`; otherwise install PostgreSQL 17 clients and use --postgres-bin PATH")
    }

    fn command(&self, name: &str) -> Command {
        local_command(self.directory.join(name))
    }
}

fn preflight(tools: &PgTools) -> Result<()> {
    shell::preflight()?;
    let result = tools
        .command("psql")
        .args([
            "-XAt",
            "-v",
            "ON_ERROR_STOP=1",
            "-c",
            "SELECT version FROM public.ultrafinance_schema",
        ])
        .output()
        .context("could not check local PostgreSQL")?;
    if !result.status.success()
        || String::from_utf8_lossy(&result.stdout).trim() != POSTGRES_SCHEMA_VERSION.to_string()
    {
        bail!(
            "local PostgreSQL is unavailable or has an incompatible schema; run database check first"
        );
    }
    Ok(())
}

fn dump_command(tools: &PgTools, path: &Path, jobs: u32, compression: &str) -> Command {
    let mut command = tools.command("pg_dump");
    command
        .args([
            "--format=directory",
            "--verbose",
            "--no-owner",
            "--no-privileges",
            "--jobs",
            &jobs.to_string(),
            "--file",
        ])
        .arg(path)
        .arg(format!("--compress={compression}"))
        .env("LC_ALL", "C");
    command
}

fn dump_failure(diagnostic: &str) -> &'static str {
    let diagnostic = diagnostic.to_lowercase();
    // Raw pg_dump diagnostics can include queries, connection details or data.
    // Return fixed descriptions only.
    if diagnostic.contains("does not support compression") {
        "this PostgreSQL client was built without the requested compression support"
    } else if diagnostic.contains("no space left") || diagnostic.contains("disk full") {
        "not enough temporary disk space; free space or set TMPDIR to a larger disk"
    } else if diagnostic.contains("server version mismatch") {
        "pg_dump is older than the local PostgreSQL server"
    } else if diagnostic.contains("permission denied") || diagnostic.contains("must be owner") {
        "insufficient local database or temporary-directory permissions"
    } else if diagnostic.contains("too many clients")
        || diagnostic.contains("remaining connection slots")
    {
        "not enough local PostgreSQL connections; reduce --dump-jobs"
    } else if diagnostic.contains("lock timeout") || diagnostic.contains("lock not available") {
        "a concurrent local schema change prevented the dump; retry after it finishes"
    } else if diagnostic.contains("connection") || diagnostic.contains("timeout expired") {
        "local PostgreSQL connectivity failed"
    } else {
        "unclassified pg_dump error; inspect local PostgreSQL logs (raw diagnostics withheld)"
    }
}

fn choose_compression(
    mut probe: impl FnMut(&str) -> Result<Option<String>>,
) -> Result<&'static str> {
    for compression in ["zstd:1", "gzip:1", "none"] {
        let Some(diagnostic) = probe(compression)? else {
            return Ok(compression);
        };
        if !diagnostic
            .to_lowercase()
            .contains("this build does not support compression with")
        {
            bail!("Local dump preflight failed: {}", dump_failure(&diagnostic));
        }
    }
    bail!("local PostgreSQL client could not create an uncompressed archive")
}

fn compression(tools: &PgTools) -> Result<&'static str> {
    choose_compression(|compression| {
        let directory = tempfile::tempdir()?;
        // A schema-only archive probes the actual binary build and local
        // connectivity without scanning table data or touching AWS.
        let result = dump_command(tools, &directory.path().join("probe"), 1, compression)
            .args(["--schema-only", "--lock-wait-timeout=30s"])
            .output()
            .context("could not probe local pg_dump compression")?;
        Ok(
            (!result.status.success())
                .then(|| String::from_utf8_lossy(&result.stderr).into_owned()),
        )
    })
}

fn run_dump(
    command: &mut Command,
    archive: &Path,
    interrupt: impl Fn() -> Result<()>,
) -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("diagnostics");
    let mut process = spawn(command.stdout(Stdio::null()).stderr(File::create(&path)?))?;
    let mut progress = DumpProgress::new(archive, File::open(&path)?);
    loop {
        interrupt()?;
        if let Some(status) = process.0.try_wait()? {
            progress.finish(status.success());
            if status.success() {
                return Ok(());
            }
            let diagnostic = fs::read_to_string(&path).unwrap_or_default();
            bail!(
                "Local dump failed ({status}): {}",
                dump_failure(&diagnostic)
            );
        }
        progress.update();
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn checksum(path: &Path) -> Result<String> {
    checksum_progress(path, None)
}

fn checksum_progress(path: &Path, progress: Option<&Counter>) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
        if let Some(progress) = progress {
            progress.inc(count as u64);
        }
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn archive_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().context("non-UTF8 archive filename")?;
        if !entry.file_type()?.is_file()
            || name.is_empty()
            || name.starts_with('.')
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        {
            bail!("unexpected entry in PostgreSQL archive");
        }
        files.push(entry.path());
    }
    files.sort();
    if !files
        .iter()
        .any(|p| p.file_name().is_some_and(|n| n == "toc.dat"))
    {
        bail!("PostgreSQL archive has no table of contents");
    }
    Ok(files)
}

fn run_upload(
    command: &mut Command,
    size: u64,
    progress: &Counter,
    interrupt: impl Fn() -> Result<()>,
) -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("upload.log");
    let log = File::create(&path)?;
    let mut process = spawn(command.stdout(log.try_clone()?).stderr(log))?;
    let mut reader = File::open(path)?;
    let mut pending = String::new();
    let mut counted = 0;
    loop {
        interrupt()?;
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        pending.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(end) = pending.find(['\r', '\n']) {
            let line: String = pending.drain(..=end).collect();
            if let Some(bytes) = uploaded_bytes(line.trim()) {
                // CLI display units are rounded; completion requires a successful
                // process exit, not a rounded "Completed 1.0 GiB" message.
                let bytes = bytes.min(size.saturating_sub(1));
                if bytes > counted {
                    progress.inc(bytes - counted);
                    counted = bytes;
                }
            }
        }
        if let Some(status) = process.0.try_wait()? {
            if !status.success() {
                bail!("S3 transfer failed; raw diagnostics withheld");
            }
            progress.inc(size - counted);
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn remote_progress(line: &str) -> Option<(&str, u64, u64, Unit)> {
    let mut fields = line.strip_prefix("UF_UPLOAD_PROGRESS:")?.split(':');
    let stage = fields.next()?;
    let done = fields.next()?.parse().ok()?;
    let total: u64 = fields.next()?.parse().ok()?;
    let unit = match (stage, fields.next()?) {
        ("download", "bytes") => Unit::Bytes,
        ("restore", "items") => Unit::Items,
        _ => return None,
    };
    if fields.next().is_some() || done > total || total == u64::MAX {
        return None;
    }
    Some((stage, done, total, unit))
}

/// Always clean up only this invocation's random prefix, including partial uploads.
struct Transfer<'a> {
    aws: &'a Aws,
    bucket: &'a str,
    prefix: String,
}
impl Transfer<'_> {
    fn uri(&self, name: &str) -> String {
        format!("s3://{}/{}/{}", self.bucket, self.prefix, name)
    }
    fn put(
        &self,
        path: &Path,
        name: &str,
        interrupts: &Interrupts,
        progress: Option<&Counter>,
    ) -> Result<String> {
        let mut command = self.aws.command();
        command
            .args(["s3", "cp"])
            .arg(path)
            .args([&self.uri(name), "--sse", "AES256"]);
        let uploaded = if let Some(progress) = progress {
            run_upload(&mut command, fs::metadata(path)?.len(), progress, || {
                interrupts.check()
            })
        } else {
            checked(command.arg("--only-show-errors"), interrupts, "S3 upload")
        };
        if uploaded.is_err() {
            interrupts.check()?;
            self.aws.call(&["sts", "get-caller-identity"])?;
            bail!("S3 upload failed; check transfer bucket permissions and network connectivity");
        }
        self.signed_url(name)
    }
    fn signed_url(&self, name: &str) -> Result<String> {
        let result = self
            .aws
            .command()
            .args(["s3", "presign", &self.uri(name), "--expires-in", "86400"])
            .output()?;
        if !result.status.success() {
            self.aws.call(&["sts", "get-caller-identity"])?;
            bail!("could not sign archive download; check bucket permissions");
        }
        let url = String::from_utf8(result.stdout)?.trim().to_owned();
        if !url.starts_with("https://") || url.contains(['\n', '\r', '\t']) {
            bail!("invalid signed S3 download URL");
        }
        Ok(url)
    }
    fn clean(&self) -> Result<()> {
        let status = self
            .aws
            .command()
            .args([
                "s3",
                "rm",
                &self.uri(""),
                "--recursive",
                "--only-show-errors",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            bail!("could not remove temporary S3 objects at {}", self.uri(""));
        }
        Ok(())
    }
}

fn parallel<T: Send>(
    items: &[PathBuf],
    jobs: u32,
    operation: impl Fn(&Path) -> Result<T> + Sync,
) -> Result<Vec<T>> {
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..(jobs as usize).min(items.len()))
            .map(|_| {
                let next = &next;
                let operation = &operation;
                scope.spawn(move || {
                    let mut results = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(path) = items.get(index) else {
                            break;
                        };
                        match operation(path) {
                            Ok(result) => results.push(result),
                            Err(error) => {
                                next.store(items.len(), Ordering::Relaxed);
                                return Err(error);
                            }
                        }
                    }
                    Ok(results)
                })
            })
            .collect();
        let mut results = Vec::new();
        // Join every worker before the caller removes the S3 prefix.
        let mut failure = None;
        for worker in workers {
            match worker.join() {
                Ok(Ok(values)) => results.extend(values),
                Ok(Err(error)) => failure = Some(error),
                Err(_) => failure = Some(anyhow::anyhow!("archive transfer worker panicked")),
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(results)
    })
}

fn remote_script(id: &str, manifest: &str, options: &Options) -> String {
    format!(
        "#!/bin/bash\nset +x\nset -Eeuo pipefail\numask 077\njob={}\nrestore_jobs={}\ntransfer_jobs={}\nmkdir -p /scratch/\"$job\"/dump\ncd /scratch/\"$job\"\ncat > downloads <<'UF_DOWNLOADS'\n{}UF_DOWNLOADS\ncat > admin-shell.sh <<'UF_ADMIN'\n{}\nUF_ADMIN\ncat > progress.py <<'UF_PROGRESS'\n{}\nUF_PROGRESS\n{}",
        quote(id),
        options.restore_jobs.unwrap_or(0),
        options.transfer_jobs,
        manifest,
        include_str!("admin-shell.sh"),
        include_str!("upload-progress.py"),
        include_str!("upload-db.sh"),
    )
}

fn execute(
    aws: &Aws,
    config: &serde_json::Value,
    task: &str,
    url: &str,
    hash: &str,
    id: &str,
    interrupts: &Interrupts,
) -> Result<()> {
    let _terminal = super::terminal::TerminalState::capture();
    let marker = format!("UF_UPLOAD_SUCCESS_{id}");
    // Only a short bootstrap enters ECS Exec's command field. No DB credentials
    // enter arguments, task definitions, logs, or local diagnostics.
    let bootstrap = format!(
        "set +x\nscript_url={}\nscript_hash={}\nscript_path={}\nsuccess_marker={}\n{}",
        quote(url),
        quote(hash),
        quote(&format!("/scratch/{id}.sh")),
        quote(&marker),
        include_str!("upload-bootstrap.sh"),
    );
    let command = format!("/bin/bash -c {}", quote(&bootstrap));
    monitor(
        aws.command().args([
            "ecs",
            "execute-command",
            "--cluster",
            text(config, "cluster")?,
            "--task",
            task,
            "--container",
            "cli",
            "--interactive",
            "--command",
            &command,
        ]),
        &marker,
        id,
        || interrupts.check(),
    )
}

fn connection_failure(diagnostic: &str) -> &'static str {
    for line in diagnostic.lines() {
        let Some(code) = line.trim().strip_prefix("UF_UPLOAD_ERROR:") else {
            continue;
        };
        return match code {
            "missing-curl" => {
                "AWS runtime image is missing curl; publish the updated runtime image or select it with --image"
            }
            "missing-pg_restore" | "missing-psql" | "missing-vacuumdb" | "postgres-version" => {
                "AWS runtime image needs PostgreSQL 17 client tools; publish the updated runtime image or select it with --image"
            }
            "missing-python3" => "AWS runtime image is missing Python 3",
            "missing-sha256sum" => "AWS runtime image is missing sha256sum",
            "bootstrap-download" => {
                "AWS task could not download its control script from S3; check signed URL validity and network access"
            }
            "bootstrap-checksum" => {
                "Downloaded control script failed its checksum; restore was not started"
            }
            "database-connectivity" => {
                "AWS task could not connect to production PostgreSQL during preflight; check database availability, credentials and TLS"
            }
            "connection-headroom" => {
                "Could not query PostgreSQL connection capacity during preflight"
            }
            "restore-capacity" => {
                "Insufficient PostgreSQL connection headroom for the requested restore workers; reduce --restore-jobs or free database connections"
            }
            "remote-script" => "Remote upload script failed; inspect the last reported stage",
            _ => "Remote bootstrap failed",
        };
    }
    if diagnostic.contains("Cannot perform start session") {
        "AWS Session Manager could not start the remote session"
    } else if diagnostic.contains("AccessDeniedException") {
        "AWS denied ECS Exec or Session Manager access"
    } else if diagnostic.contains("TargetNotConnectedException") {
        "ECS Exec agent was not connected"
    } else {
        "Production upload failed or disconnected without a success marker"
    }
}

fn monitor(
    command: &mut Command,
    marker: &str,
    id: &str,
    interrupt: impl Fn() -> Result<()>,
) -> Result<()> {
    let directory = tempfile::tempdir()?;
    let log_path = directory.path().join("exec.log");
    let log = File::create(&log_path)?;
    let mut process = spawn(
        command
            .stdin(Stdio::inherit())
            .stdout(log.try_clone()?)
            .stderr(log),
    )?;
    let _foreground = super::terminal::ForegroundGroup::give_to(process.0.id())
        .context("could not give ECS Exec foreground terminal access")?;
    let mut reader = File::open(&log_path)?;
    let mut position = 0;
    let mut pending = String::new();
    let mut succeeded = false;
    let mut progress: Option<Progress> = None;
    let mut remote_stage = String::new();
    let mut exited: Option<std::process::ExitStatus> = None;
    loop {
        interrupt()?;
        reader.seek(SeekFrom::Start(position))?;
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        position += bytes.len() as u64;
        pending.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(end) = pending.find('\n') {
            let line: String = pending.drain(..=end).collect();
            let line = line.trim();
            if line == marker {
                succeeded = true;
            }
            if let Some(stage) = line.strip_prefix("UF_UPLOAD_STAGE:")
                && matches!(
                    stage,
                    "bootstrap"
                        | "preflight"
                        | "download"
                        | "restore"
                        | "permissions"
                        | "analyze"
                        | "validate"
                        | "cutover"
                        | "delete-old"
                        | "complete"
                )
            {
                if let Some(mut previous) = progress.take() {
                    previous.finish();
                }
                remote_stage = stage.to_owned();
                let label = match stage {
                    "bootstrap" => "Bootstrap",
                    "download" => "Download",
                    "restore" => "Restore",
                    "analyze" => "Analyze",
                    "permissions" => "Permissions",
                    "validate" => "Validate",
                    "cutover" => "Cutover",
                    "delete-old" => "Delete old DB",
                    "complete" => "Complete",
                    _ => "Preflight",
                };
                let unit = if stage == "download" {
                    Unit::Bytes
                } else {
                    Unit::Items
                };
                progress = Some(Progress::new(&[(label, None, unit)]));
            }
            if let Some((stage, done, total, _unit)) = remote_progress(line)
                && stage == remote_stage
                && let Some(progress) = &progress
            {
                let counter = progress.counter(0);
                counter.set_total(total);
                if counter.total() == Some(total) {
                    counter.set(done);
                }
            }
        }
        if let Some(status) = exited {
            // The session plugin can exit 0 when the remote command failed. A
            // successful ECS response or process status is not proof of restore.
            if status.success() && succeeded {
                if let Some(mut progress) = progress.take() {
                    progress.finish();
                }
                return Ok(());
            }
            let diagnostic = fs::read_to_string(&log_path).unwrap_or_default();
            let reason = connection_failure(&diagnostic);
            let saved = directory.keep();
            bail!(
                "{reason}. Private connection log: {}. Inspect production before retrying; cutover databases: uf_upload_{id} / uf_old_{id}",
                saved.join("exec.log").display(),
            );
        }
        // Drain once more after exit: the final marker can arrive between the
        // preceding read and try_wait().
        exited = process.0.try_wait()?;
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct ArchiveEntry {
    name: String,
    sha256: String,
    size: u64,
}

fn validate_entries(entries: &[ArchiveEntry]) -> Result<()> {
    let mut names = std::collections::HashSet::new();
    for entry in entries {
        if entry.name.is_empty()
            || entry.name.starts_with('.')
            || !entry
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            || matches!(entry.name.as_str(), "upload.sh" | "manifest.json")
            || entry.sha256.len() != 64
            || !entry.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !names.insert(&entry.name)
        {
            bail!("invalid or duplicate archive manifest entry");
        }
    }
    if !names.contains(&"toc.dat".to_owned()) {
        bail!("archive manifest is missing toc.dat");
    }
    Ok(())
}

/// Read only the data heredoc from older upload scripts; never execute downloaded code.
fn manifest_entries(
    manifest: &str,
    sizes: Option<&std::collections::HashMap<String, u64>>,
) -> Result<Vec<ArchiveEntry>> {
    let mut entries = Vec::new();
    for line in manifest.lines().filter(|line| !line.is_empty()) {
        let fields: Vec<_> = line.split('\t').collect();
        let (name, sha256, size) = match fields.as_slice() {
            [name, hash, size, _url] => (
                *name,
                *hash,
                size.parse::<u64>().context("invalid archive size")?,
            ),
            [name, hash, _url] => (
                *name,
                *hash,
                *sizes
                    .and_then(|sizes| sizes.get(*name))
                    .context("legacy archive object is missing")?,
            ),
            _ => bail!("invalid archive manifest"),
        };
        entries.push(ArchiveEntry {
            name: name.to_owned(),
            sha256: sha256.to_owned(),
            size,
        });
    }
    validate_entries(&entries)?;
    Ok(entries)
}

fn legacy_manifest(script: &str) -> Result<&str> {
    script
        .split_once("cat > downloads <<'UF_DOWNLOADS'\n")
        .and_then(|(_, body)| body.split_once("\nUF_DOWNLOADS\n").map(|(data, _)| data))
        .context("legacy upload script has no recognized archive manifest")
}

fn resume(
    aws: &Aws,
    config: &serde_json::Value,
    bucket: &str,
    id: &str,
    options: &Options,
    interrupts: &Interrupts,
) -> Result<()> {
    if id.len() != 32
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("--resume requires the 32-character transfer ID printed by upload-db");
    }
    let transfer = Transfer {
        aws,
        bucket,
        prefix: format!("ultrafinance-upload/{id}"),
    };
    let directory = tempfile::tempdir()?;
    let listing = aws.call(&[
        "s3api",
        "list-objects-v2",
        "--bucket",
        bucket,
        "--prefix",
        &format!("{}/", transfer.prefix),
    ])?;
    let mut sizes = std::collections::HashMap::new();
    for object in listing["Contents"]
        .as_array()
        .context("S3 transfer is missing or has expired")?
    {
        let key = text(object, "Key")?;
        if let Some(name) = key.strip_prefix(&format!("{}/", transfer.prefix)) {
            sizes.insert(
                name.to_owned(),
                object["Size"].as_u64().context("missing S3 object size")?,
            );
        }
    }
    let entries: Vec<ArchiveEntry> = if sizes.contains_key("manifest.json") {
        let path = directory.path().join("manifest.json");
        aws.call(&[
            "s3api",
            "get-object",
            "--bucket",
            bucket,
            "--key",
            &format!("{}/manifest.json", transfer.prefix),
            path.to_str().context("invalid temporary path")?,
        ])?;
        serde_json::from_slice(&fs::read(path)?).context("invalid saved archive manifest")?
    } else {
        let path = directory.path().join("legacy-upload.sh");
        aws.call(&[
            "s3api",
            "get-object",
            "--bucket",
            bucket,
            "--key",
            &format!("{}/upload.sh", transfer.prefix),
            path.to_str().context("invalid temporary path")?,
        ])?;
        let script = fs::read_to_string(path)?;
        manifest_entries(legacy_manifest(&script)?, Some(&sizes))?
    };
    validate_entries(&entries)?;
    let mut total = 0u64;
    for entry in &entries {
        if sizes.get(&entry.name) != Some(&entry.size) {
            bail!("S3 archive is incomplete or an object size changed; refusing to resume");
        }
        total = total
            .checked_add(entry.size)
            .context("archive size overflow")?;
    }
    if options.check_only {
        println!(
            "S3 resume archive validated: {} files, {:.2} GiB. No task launched or production changes made.",
            entries.len(),
            total as f64 / 1073741824.0
        );
        return Ok(());
    }
    eprintln!(
        "Resuming {} archive files ({:.2} GiB) from S3. No local dump or archive upload. Stop the original uploader before continuing this resume command.",
        entries.len(),
        total as f64 / 1073741824.0
    );
    let mut signing = Progress::new(&[("Prepare resume", Some(entries.len() as u64), Unit::Items)]);
    let signed = signing.counter(0);
    let names: Vec<_> = entries
        .iter()
        .map(|entry| PathBuf::from(&entry.name))
        .collect();
    let mut downloads = parallel(&names, options.transfer_jobs, |name| {
        interrupts.check()?;
        let name = name.to_str().context("invalid archive name")?;
        let entry = entries
            .iter()
            .find(|entry| entry.name == name)
            .context("archive entry missing")?;
        let url = transfer.signed_url(name)?;
        signed.inc(1);
        Ok(format!(
            "{}\t{}\t{}\t{}\n",
            entry.name, entry.sha256, entry.size, url
        ))
    })?;
    signing.finish();
    downloads.sort();
    // Fresh names avoid collisions with scratch/staging databases from the old
    // execution. Always regenerate short-lived URLs and use the current script.
    let job = uuid::Uuid::new_v4().simple().to_string();
    let path = directory.path().join("resume.sh");
    fs::write(&path, remote_script(&job, &downloads.concat(), options))?;
    let name = format!("resume-{job}.sh");
    let hash = checksum(&path)?;
    let url = transfer.put(&path, &name, interrupts, None)?;
    let outcome = shell::run_shell(
        aws,
        config,
        options.image_selection(),
        Duration::from_secs(300),
        || interrupts.check(),
        |task| execute(aws, config, task, &url, &hash, &job, interrupts),
    );
    if let Err(error) = outcome {
        eprintln!("S3 archive retained for resume ID {id}; inspect production before retrying.");
        return Err(error);
    }
    transfer.clean()?;
    println!(
        "S3 archive restored to production and validated. Old database and transfer objects deleted."
    );
    Ok(())
}

pub(super) fn run(root: &Path, options: Options) -> Result<()> {
    if !options.check_only {
        shell::preflight()?;
    }
    if let Some(image) = &options.image {
        super::aws::validate_image(image)?;
    }
    let config = shell::config(root)?;
    let bucket = options
        .bucket
        .as_deref()
        .or_else(|| config["upload_bucket"].as_str())
        .context("missing upload bucket; apply the CLI infrastructure or pass --bucket NAME")?;
    // Restrict to ordinary bucket names, not S3 URIs, access points or flags.
    if bucket.len() < 3
        || bucket.len() > 63
        || !bucket
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b".-".contains(&c))
        || !bucket.starts_with(|c: char| c.is_ascii_alphanumeric())
        || !bucket.ends_with(|c: char| c.is_ascii_alphanumeric())
    {
        bail!("--bucket must be an existing private S3 bucket name");
    }
    let interrupts = Interrupts::install()?;
    let aws = Aws::configured(root)?;
    aws.call(&["sts", "get-caller-identity"])?;
    let location = aws.call(&["s3api", "get-bucket-location", "--bucket", bucket])?;
    let region = location["LocationConstraint"]
        .as_str()
        .unwrap_or("us-east-1");
    if region != aws.region {
        bail!(
            "transfer bucket must be in the configured AWS region {}",
            aws.region
        );
    }
    let block = aws.call(&["s3api", "get-public-access-block", "--bucket", bucket])?;
    if ![
        "BlockPublicAcls",
        "IgnorePublicAcls",
        "BlockPublicPolicy",
        "RestrictPublicBuckets",
    ]
    .iter()
    .all(|key| block["PublicAccessBlockConfiguration"][key] == true)
    {
        bail!("transfer bucket must enable all four S3 Block Public Access settings");
    }
    let versioning = aws.call(&["s3api", "get-bucket-versioning", "--bucket", bucket])?;
    if versioning["Status"] == "Enabled" {
        bail!(
            "transfer bucket must not enable versioning, so cleanup removes the actual dump data"
        );
    }
    if let Some(id) = &options.resume {
        return resume(&aws, &config, bucket, id, &options, &interrupts);
    }
    let tools = PgTools::resolve(options.postgres_bin.as_deref())?;
    preflight(&tools)?;
    let compression = compression(&tools)?;
    let directory = tempfile::tempdir()?;
    let dump = directory.path().join("dump");
    let dump_jobs = options.dump_jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map_or(1, |n| n.get() as u32)
            .min(128)
    });
    eprintln!(
        "Replacing production from local PostgreSQL; the old production database will be deleted after cutover."
    );
    eprintln!("Dumping a consistent local snapshot with {dump_jobs} workers ({compression})...");
    run_dump(
        &mut dump_command(&tools, &dump, dump_jobs, compression),
        &dump,
        || interrupts.check(),
    )?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let transfer = Transfer {
        aws: &aws,
        bucket,
        prefix: format!("ultrafinance-upload/{id}"),
    };
    let mut resumable = false;
    let outcome = (|| {
        eprintln!(
            "Uploading archive with {} workers...",
            options.transfer_jobs
        );
        let files = archive_files(&dump)?;
        let total = files.iter().try_fold(0u64, |sum, path| {
            Ok::<_, std::io::Error>(sum + fs::metadata(path)?.len())
        })?;
        let mut progress = Progress::new(&[
            ("Checksum", Some(total), Unit::Bytes),
            ("Upload", Some(total), Unit::Bytes),
        ]);
        let hashed = progress.counter(0);
        let uploaded = progress.counter(1);
        let mut entries = parallel(&files, options.transfer_jobs, |path| {
            interrupts.check()?;
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .context("invalid archive filename")?;
            let size = fs::metadata(path)?.len();
            let hash = checksum_progress(path, Some(&hashed))?;
            let url = transfer.put(path, name, &interrupts, Some(&uploaded))?;
            Ok(format!("{name}\t{hash}\t{size}\t{url}\n"))
        })?;
        progress.finish();
        entries.sort();
        let archived = manifest_entries(&entries.concat(), None)?;
        let manifest = directory.path().join("manifest.json");
        fs::write(&manifest, serde_json::to_vec(&archived)?)?;
        transfer.put(&manifest, "manifest.json", &interrupts, None)?;
        resumable = true;
        eprintln!("Resume ID: {id}");
        let script = directory.path().join("upload.sh");
        fs::write(&script, remote_script(&id, &entries.concat(), &options))?;
        let hash = checksum(&script)?;
        let url = transfer.put(&script, "upload.sh", &interrupts, None)?;
        shell::run_shell(
            &aws,
            &config,
            options.image_selection(),
            Duration::from_secs(300),
            || interrupts.check(),
            |task| execute(&aws, &config, task, &url, &hash, &id, &interrupts),
        )
    })();
    if outcome.is_err() && resumable {
        eprintln!(
            "Uploaded archive retained. After checking production, resume with: cargo run --release -- infra upload-db --resume {id} --bucket {bucket}"
        );
        return outcome;
    }
    let mut cleanup_progress = Progress::new(&[("S3 cleanup", None, Unit::Items)]);
    let cleanup = transfer.clean();
    if cleanup.is_ok() {
        cleanup_progress.finish();
    }
    drop(cleanup_progress);
    if let Err(error) = &cleanup {
        eprintln!("Temporary transfer cleanup failed: {error}");
    }
    outcome?;
    cleanup?;
    println!(
        "Local database uploaded to production, validated and analyzed. Old production database deleted."
    );
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/infra/upload_db/tests.rs"]
mod tests;
