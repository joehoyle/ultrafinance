use super::{
    aws::{Api, Aws, text},
    import_db::{database_environment, spawn, target},
    shell::{self, ImageSelection, Interrupts},
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn connection(
    host: &str,
    port: u16,
    database: &str,
    username: &str,
    password: &str,
    read_only: bool,
) -> Result<String> {
    let mut url = reqwest::Url::parse(&format!("postgresql://127.0.0.1:{port}"))?;
    url.set_host(Some(host)).context("invalid database host")?;
    // URL user-info setters preserve percent escapes; encode literal percent
    // signs first so a password containing "%40" is not changed to "@".
    url.set_username(&username.replace('%', "%25"))
        .map_err(|_| anyhow::anyhow!("invalid database username"))?;
    url.set_password(Some(&password.replace('%', "%25")))
        .map_err(|_| anyhow::anyhow!("invalid database password"))?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("invalid database URL"))?
        .push(database);
    url.query_pairs_mut().append_pair("sslmode", "require");
    // Preserve the RDS hostname for certificate verification while connecting
    // through the loopback tunnel, without changing local DNS configuration.
    url.query_pairs_mut().append_pair("hostaddr", "127.0.0.1");
    if read_only {
        url.query_pairs_mut()
            .append_pair("options", "-c default_transaction_read_only=on");
    }
    // PostgreSQL URLs percent-decode spaces but do not decode form-style '+'.
    let query = url.query().map(|query| query.replace('+', "%20"));
    url.set_query(query.as_deref());
    Ok(url.into())
}

pub(super) fn run(root: &Path, read_only: bool, command: &[OsString]) -> Result<()> {
    let (program, arguments) = command
        .split_first()
        .context("a local command is required")?;
    if !std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .any(|path| path.join("session-manager-plugin").is_file())
    {
        bail!("install session-manager-plugin and add it to PATH before connecting");
    }
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
                json!({"host":[host], "portNumber":["5432"], "localPortNumber":[port.to_string()]})
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
            let url = connection(&host, port, &database, username, password, read_only)?;
            let certificate_directory = tempfile::tempdir()?;
            let certificates = certificate_directory.path().join("rds-ca.pem");
            std::fs::write(
                &certificates,
                [
                    include_bytes!("../../../../deploy/rds-ca/ca-central-1-1.crt").as_slice(),
                    include_bytes!("../../../../deploy/rds-ca/ca-central-1-2.crt").as_slice(),
                    include_bytes!("../../../../deploy/rds-ca/ca-central-1-3.crt").as_slice(),
                ]
                .concat(),
            )?;
            eprintln!(
                "Running local command against production via 127.0.0.1:{port}{}",
                if read_only { " (read-only)" } else { "" }
            );
            let mut local = Command::new(program);
            local
                .args(arguments)
                .env("ULTRAFINANCE_DATABASE_URL", &url)
                .env("PGHOST", &host)
                .env("PGHOSTADDR", "127.0.0.1")
                .env("PGPORT", port.to_string())
                .env("PGDATABASE", &database)
                .env("PGUSER", username)
                .env("PGPASSWORD", password)
                .env("PGSSLMODE", "verify-full")
                .env("PGSSLROOTCERT", &certificates)
                .env("PGCONNECT_TIMEOUT", "35")
                .env_remove("PGSERVICE")
                .env_remove("PGSERVICEFILE")
                .env_remove("PGOPTIONS");
            if read_only {
                local.env("PGOPTIONS", "-c default_transaction_read_only=on");
            }
            let mut child = spawn(&mut local)?;
            loop {
                interrupts.check()?;
                if let Some(status) = child.0.try_wait()? {
                    if !status.success() {
                        bail!("local command failed ({status})");
                    }
                    return Ok(());
                }
                if tunnel.0.try_wait()?.is_some() {
                    bail!("production database tunnel disconnected");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connection_encodes_credentials_and_database_and_can_reject_writes() {
        let url = reqwest::Url::parse(
            &connection(
                "database.internal",
                12345,
                "db/name?#",
                "user%@name",
                "p@ss:/?#%40",
                true,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(url.host_str(), Some("database.internal"));
        assert_eq!(url.port(), Some(12345));
        assert_eq!(url.path(), "/db%2Fname%3F%23");
        assert!(!url.username().contains('@'));
        assert!(url.username().contains("%25"));
        assert!(url.password().unwrap().ends_with("%2540"));
        let options: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(options["sslmode"], "require");
        assert_eq!(options["hostaddr"], "127.0.0.1");
        assert_eq!(options["options"], "-c default_transaction_read_only=on");
        assert!(
            url.query()
                .unwrap()
                .contains("-c%20default_transaction_read_only")
        );
        assert!(
            !connection("database.internal", 12345, "db", "user", "pass", false)
                .unwrap()
                .contains("options")
        );
    }
}
