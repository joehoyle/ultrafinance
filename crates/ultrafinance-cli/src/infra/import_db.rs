use super::{
    aws::{Api, Aws, text},
    shell::{self, ImageSelection, Interrupts},
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use ultrafinance_core::store::LOCAL_DATABASE_URL;

// A separate process group also includes the Session Manager plugin started by AWS CLI.
pub(super) struct Process(pub(super) Child);
impl Drop for Process {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGTERM);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(super) fn spawn(command: &mut Command) -> Result<Process> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    Ok(Process(
        command.spawn().context("could not start import process")?,
    ))
}

pub(super) fn checked(command: &mut Command, interrupts: &Interrupts, label: &str) -> Result<()> {
    // Database diagnostics can contain data or connection details; do not echo them.
    let mut child = spawn(command.stdout(Stdio::null()).stderr(Stdio::null()))?;
    loop {
        interrupts.check()?;
        if let Some(status) = child.0.try_wait()? {
            if !status.success() {
                bail!(
                    "{label} failed ({status}); check PostgreSQL connectivity, permissions and client/server versions"
                );
            }
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub(super) fn local_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command
        .args(["--no-password", "--dbname", LOCAL_DATABASE_URL])
        .env("PGCONNECT_TIMEOUT", "35")
        .env_remove("PGPASSWORD")
        .env_remove("PGOPTIONS")
        .env_remove("PGSERVICE");
    command
}

fn preflight() -> Result<()> {
    for program in ["pg_dump", "pg_restore", "psql", "session-manager-plugin"] {
        if !std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .any(|path| path.join(program).is_file())
        {
            bail!("install {program} and add it to PATH before importing");
        }
    }
    let version = Command::new("pg_dump").arg("--version").output()?;
    let major = String::from_utf8_lossy(&version.stdout)
        .split_whitespace()
        .find_map(|part| part.split('.').next()?.parse::<u32>().ok());
    if !version.status.success() || major.is_none_or(|major| major < 17) {
        bail!("pg_dump 17 or newer is required; put current PostgreSQL client tools on PATH");
    }
    let result = local_command("psql")
        .args(["-X", "-A", "-t", "-v", "ON_ERROR_STOP=1", "-c", "SELECT 1"])
        .output()
        .context(
            "could not check local PostgreSQL; start it with docker compose up -d --wait postgres",
        )?;
    if !result.status.success() {
        bail!(
            "cannot connect to local PostgreSQL on 127.0.0.1:55432; run docker compose up -d --wait postgres"
        );
    }
    Ok(())
}

pub(super) fn target(cluster: &str, task: &str, response: &Value) -> Result<String> {
    let runtime = response["tasks"][0]["containers"]
        .as_array()
        .and_then(|containers| containers.iter().find(|c| c["name"] == "cli"))
        .and_then(|c| c["runtimeId"].as_str())
        .context("CLI container runtime ID missing")?;
    Ok(format!(
        "ecs:{}_{}_{}",
        cluster.rsplit('/').next().unwrap_or(cluster),
        task.rsplit('/').next().unwrap_or(task),
        runtime
    ))
}

pub(super) fn database_environment(definition: &Value, name: &str) -> Result<String> {
    definition["taskDefinition"]["containerDefinitions"]
        .as_array()
        .and_then(|containers| containers.iter().find(|c| c["name"] == "cli"))
        .and_then(|c| c["environment"].as_array())
        .and_then(|env| env.iter().find(|v| v["name"] == name))
        .and_then(|v| v["value"].as_str())
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .with_context(|| format!("CLI task missing {name}; apply CLI infrastructure"))
}

fn restore_command(path: &Path) -> Command {
    let mut command = local_command("pg_restore");
    command
        .args([
            "--clean",
            "--if-exists",
            "--no-owner",
            "--no-privileges",
            "--single-transaction",
            "--exit-on-error",
        ])
        .arg(path);
    command
}

pub fn run(root: &Path) -> Result<()> {
    preflight()?;
    let interrupts = Interrupts::install()?;
    let config = shell::config(root)?;
    let aws = Aws::configured(root)?;
    let definition = aws.call(&[
        "ecs",
        "describe-task-definition",
        "--task-definition",
        text(&config, "task_definition")?,
    ])?;
    let host = database_environment(&definition, "ULTRAFINANCE_DATABASE_HOST")?;
    let database = database_environment(&definition, "ULTRAFINANCE_DATABASE_NAME")?;
    let secret = aws.call(&[
        "secretsmanager",
        "get-secret-value",
        "--secret-id",
        text(&config, "database_secret")?,
    ])?;
    let credentials: Value = serde_json::from_str(text(&secret, "SecretString")?)
        .context("invalid database administrator secret")?;
    let username = text(&credentials, "username")?;
    let password = text(&credentials, "password")?;
    eprintln!("Copying production into local PostgreSQL at 127.0.0.1:55432/ultrafinance...");
    shell::run_shell(
        &aws,
        &config,
        ImageSelection::Live,
        Duration::from_secs(300),
        || interrupts.check(),
        |task| {
            let cluster = text(&config, "cluster")?;
            let response = aws.call(&[
                "ecs",
                "describe-tasks",
                "--cluster",
                cluster,
                "--tasks",
                task,
            ])?;
            let target = target(cluster, task, &response)?;
            let listener = TcpListener::bind("127.0.0.1:0")?;
            let port = listener.local_addr()?.port();
            drop(listener);
            let parameters =
                json!({"host":[host],"portNumber":["5432"],"localPortNumber":[port.to_string()]})
                    .to_string();
            let mut tunnel = spawn(
                aws.command()
                    .args([
                        "ssm",
                        "start-session",
                        "--target",
                        &target,
                        "--document-name",
                        "AWS-StartPortForwardingSessionToRemoteHost",
                        "--parameters",
                        &parameters,
                    ])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null()),
            )?;
            let deadline = Instant::now() + Duration::from_secs(60);
            loop {
                interrupts.check()?;
                if tunnel.0.try_wait()?.is_some() {
                    bail!(
                        "SSM tunnel failed; check ssm:StartSession permissions and Session Manager plugin"
                    );
                }
                if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    break;
                }
                if Instant::now() >= deadline {
                    bail!("timed out opening production database tunnel");
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            let directory = tempfile::tempdir()?;
            let dump = directory.path().join("production.dump");
            eprintln!("Downloading a consistent production snapshot...");
            checked(
                Command::new("pg_dump")
                    .args([
                        "--no-password",
                        "--host",
                        "127.0.0.1",
                        "--port",
                        &port.to_string(),
                        "--username",
                        username,
                        "--dbname",
                        &database,
                        "--format=custom",
                        "--no-owner",
                        "--no-privileges",
                        "--file",
                    ])
                    .arg(&dump)
                    .env("PGPASSWORD", password)
                    .env("PGSSLMODE", "require")
                    .env("PGCONNECT_TIMEOUT", "35")
                    .env_remove("PGOPTIONS")
                    .env_remove("PGSERVICE"),
                &interrupts,
                "Production dump",
            )?;
            drop(tunnel);
            interrupts.check()?;
            // Check the local server is still reachable before starting the restore.
            preflight()?;
            eprintln!("Restoring local database in one transaction...");
            checked(
                &mut restore_command(&dump),
                &interrupts,
                "Local restore (rolled back)",
            )?;
            println!(
                "Production database imported locally. Run ultrafinance merchants stats to inspect it."
            );
            Ok(())
        },
    )
}

#[cfg(test)]
#[path = "../../tests/unit/infra/import_db/tests.rs"]
mod tests;
