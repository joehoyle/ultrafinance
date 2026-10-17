use super::aws::{Api, Aws, text, validate_image};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    io::{self, IsTerminal, Write},
    path::Path,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

struct StartupIndicator {
    updates: Option<mpsc::Sender<String>>,
    worker: Option<JoinHandle<()>>,
}

impl StartupIndicator {
    fn start() -> Self {
        let initial = "Waiting for Fargate task".to_owned();
        if !io::stderr().is_terminal() {
            eprintln!("{initial} and ECS Exec agent...");
            return Self {
                updates: None,
                worker: None,
            };
        }
        let (updates, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let started = Instant::now();
            let mut message = initial;
            let frames = ['|', '/', '-', '\\'];
            let mut frame = 0;
            loop {
                {
                    let mut output = io::stderr().lock();
                    let _ = write!(
                        output,
                        "\r\x1b[2K{} {message} ({}s)",
                        frames[frame % frames.len()],
                        started.elapsed().as_secs()
                    );
                    let _ = output.flush();
                }
                frame += 1;
                match receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok(update) => message = update,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            let mut output = io::stderr().lock();
            let _ = write!(output, "\r\x1b[2K");
            let _ = output.flush();
        });
        Self {
            updates: Some(updates),
            worker: Some(worker),
        }
    }

    fn update(&self, status: &str) {
        if let Some(updates) = &self.updates {
            let message = if status == "RUNNING" {
                "Task running; waiting for ECS Exec agent".to_owned()
            } else {
                format!("Waiting for Fargate task: {status}")
            };
            let _ = updates.send(message);
        }
    }
}

impl Drop for StartupIndicator {
    fn drop(&mut self) {
        self.updates.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(unix)]
static INTERRUPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(unix)]
extern "C" fn on_interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub(super) struct Interrupts {
    #[cfg(unix)]
    previous: libc::sigaction,
}

impl Interrupts {
    pub(super) fn install() -> Result<Self> {
        #[cfg(unix)]
        {
            INTERRUPTED.store(false, std::sync::atomic::Ordering::Relaxed);
            // A caught handler is reset to default when the AWS child execs, so
            // Ctrl-C still reaches remote commands instead of killing this launcher.
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = on_interrupt as *const () as usize;
            action.sa_flags = libc::SA_RESTART;
            let mut previous = unsafe { std::mem::zeroed() };
            if unsafe { libc::sigaction(libc::SIGINT, &action, &mut previous) } != 0 {
                return Err(io::Error::last_os_error().into());
            }
            Ok(Self { previous })
        }
        #[cfg(not(unix))]
        bail!("production shell currently requires a Unix terminal")
    }

    pub(super) fn check(&self) -> Result<()> {
        #[cfg(unix)]
        if INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed) {
            bail!("shell startup interrupted");
        }
        Ok(())
    }
}

impl Drop for Interrupts {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::sigaction(libc::SIGINT, &self.previous, std::ptr::null_mut());
        }
    }
}

fn session_command() -> String {
    let script = include_str!("admin-shell.sh");
    format!("/bin/bash -c '{}'", script.replace('\'', "'\"'\"'"))
}

#[derive(Clone, Copy)]
pub(super) enum SessionMode {
    Temporary,
    Persistent { hours: u32 },
}

pub(super) fn preflight() -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("run infra cli or cli-attach in an interactive terminal");
    }
    if !std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .any(|path| path.join("session-manager-plugin").is_file())
    {
        bail!("install the AWS Session Manager plugin before opening a shell");
    }
    Ok(())
}

fn persistent_command() -> String {
    let script = include_str!("admin-shell.sh");
    let tmux = include_str!("persistent-shell.sh");
    format!(
        "/bin/bash -c '{}' admin-shell /bin/bash -c '{}'",
        script.replace('\'', "'\"'\"'"),
        tmux.replace('\'', "'\"'\"'")
    )
}

pub(super) fn execute(aws: &Aws, config: &Value, task: &str, persistent: bool) -> Result<()> {
    let _interrupts = Interrupts::install()?;
    let _terminal = super::terminal::TerminalState::capture();
    let command = if persistent {
        persistent_command()
    } else {
        session_command()
    };
    super::checked(aws.command().args([
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
    ]))
}

fn registration(definition: &Value, image: &str) -> Result<Value> {
    validate_image(image)?;
    let mut request = serde_json::Map::new();
    for field in [
        "family",
        "taskRoleArn",
        "executionRoleArn",
        "networkMode",
        "containerDefinitions",
        "volumes",
        "requiresCompatibilities",
        "cpu",
        "memory",
        "runtimePlatform",
        "ephemeralStorage",
    ] {
        if let Some(value) = definition.get(field) {
            request.insert(field.into(), value.clone());
        }
    }
    let cli = request
        .get_mut("containerDefinitions")
        .and_then(Value::as_array_mut)
        .and_then(|items| items.iter_mut().find(|item| item["name"] == "cli"))
        .context("task definition missing cli container")?;
    let names: Vec<_> = cli["secrets"]
        .as_array()
        .context("CLI administrator secrets missing; apply CLI infrastructure")?
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();
    if !["ULTRAFINANCE_ADMIN_USERNAME", "ULTRAFINANCE_ADMIN_PASSWORD"]
        .iter()
        .all(|name| names.contains(name))
    {
        bail!("CLI administrator credentials missing; apply CLI infrastructure");
    }
    cli["image"] = json!(image);
    Ok(Value::Object(request))
}

pub(super) enum ImageSelection<'a> {
    Live,
    Latest,
    Digest(&'a str),
}

fn resolve_image(aws: &impl Api, function: &str, selection: ImageSelection<'_>) -> Result<String> {
    let qualifier = match selection {
        ImageSelection::Digest(image) => return Ok(image.to_owned()),
        ImageSelection::Live => "live".to_owned(),
        ImageSelection::Latest => {
            let versions = aws.call(&[
                "lambda",
                "list-versions-by-function",
                "--function-name",
                function,
                "--query",
                "Versions[].Version",
            ])?;
            versions
                .as_array()
                .context("published Lambda version list missing")?
                .iter()
                .filter_map(|v| v.as_str()?.parse::<u64>().ok())
                .filter(|v| *v > 0)
                .max()
                .context("no published Lambda versions are available")?
                .to_string()
        }
    };
    aws.call(&[
        "lambda",
        "get-function",
        "--function-name",
        function,
        "--qualifier",
        &qualifier,
        "--query",
        "Code.ResolvedImageUri",
    ])?
    .as_str()
    .context("Lambda image digest missing")
    .map(str::to_owned)
}

pub(super) fn open(
    root: &Path,
    image: ImageSelection<'_>,
    mode: SessionMode,
    snapshot: Option<&str>,
) -> Result<()> {
    preflight()?;
    let mut config = config(root)?;
    let aws = Aws::configured(root)?;
    if let Some(snapshot) = snapshot {
        super::persistent::restore_snapshot(&aws, &mut config, snapshot)?;
    }
    let interrupts = Interrupts::install()?;
    run_shell_mode(
        &aws,
        &config,
        image,
        mode,
        Duration::from_secs(300),
        || interrupts.check(),
        |task| {
            if matches!(mode, SessionMode::Temporary) {
                println!(
                    "Opening database administrator shell. Ctrl-C cancels remote commands; use exit to stop the task. The task expires at its configured lifetime; scratch files are deleted when it stops."
                );
            } else {
                println!(
                    "Opening persistent tmux administrator shell. Detach with Ctrl-B then D. Disconnecting keeps the task and disk until expiry or cli-stop."
                );
            }
            io::stdout().flush()?;
            execute(
                &aws,
                &config,
                task,
                matches!(mode, SessionMode::Persistent { .. }),
            )
        },
    )
}

pub(super) fn config(root: &Path) -> Result<Value> {
    let result = std::process::Command::new("tofu")
        .current_dir(root)
        .args(["-chdir=infra", "output", "-json", "cli_runner"])
        .output()?;
    if !result.status.success() {
        bail!("missing cli_runner infrastructure output");
    }
    let config: Value = serde_json::from_slice(&result.stdout)?;
    if config.is_null() {
        bail!("apply the Aurora and application infrastructure first");
    }
    Ok(config)
}

pub(super) fn run_shell(
    aws: &impl Api,
    config: &Value,
    image: ImageSelection<'_>,
    timeout: Duration,
    interrupt: impl Fn() -> Result<()>,
    session: impl FnOnce(&str) -> Result<()>,
) -> Result<()> {
    run_shell_mode(
        aws,
        config,
        image,
        SessionMode::Temporary,
        timeout,
        interrupt,
        session,
    )
}

pub(super) fn run_shell_mode(
    aws: &impl Api,
    config: &Value,
    image: ImageSelection<'_>,
    mode: SessionMode,
    timeout: Duration,
    interrupt: impl Fn() -> Result<()>,
    session: impl FnOnce(&str) -> Result<()>,
) -> Result<()> {
    if let SessionMode::Persistent { hours } = mode
        && !(1..=168).contains(&hours)
    {
        bail!("persistent task lifetime must be between 1 and 168 hours");
    }
    let cluster = text(config, "cluster")?;
    let job = uuid::Uuid::new_v4().simple().to_string();
    let mut task: Option<String> = None;
    let mut revision: Option<String> = None;
    let mut launch_attempted = false;
    let mut retain = false;
    let outcome = (|| -> Result<()> {
        interrupt()?;
        let definition = aws.call(&[
            "ecs",
            "describe-task-definition",
            "--task-definition",
            text(config, "task_definition")?,
        ])?;
        let chosen = resolve_image(aws, text(config, "function_name")?, image)?;
        let mut request = registration(&definition["taskDefinition"], &chosen)?;
        if let SessionMode::Persistent { hours } = mode {
            let cli = request["containerDefinitions"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|container| container["name"] == "cli")
                .unwrap();
            cli["command"] = json!([format!(
                "mkdir -p /scratch/tmp && exec sleep {}",
                hours * 3600
            )]);
            if cli["environment"].is_null() {
                cli["environment"] = json!([]);
            }
            let environment = cli["environment"]
                .as_array_mut()
                .context("invalid CLI environment")?;
            environment.retain(|value| value["name"] != "ULTRAFINANCE_CLI_PERSISTENT");
            environment.push(json!({"name":"ULTRAFINANCE_CLI_PERSISTENT","value":"1"}));
        }
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("task.json");
        std::fs::write(&path, serde_json::to_vec(&request)?)?;
        let registered = aws.call(&[
            "ecs",
            "register-task-definition",
            "--cli-input-json",
            &format!("file://{}", path.display()),
        ])?;
        revision = Some(text(&registered["taskDefinition"], "taskDefinitionArn")?.to_owned());
        interrupt()?;
        let network = json!({"awsvpcConfiguration":{"subnets": config["subnets"],
            "securityGroups":config["security_groups"],"assignPublicIp":"DISABLED"}})
        .to_string();
        println!("Session: {job}");
        if let SessionMode::Persistent { hours } = mode {
            println!(
                "Maximum lifetime: {hours} hours from task startup; reconnecting does not extend it.\nReconnect: cargo run -- infra cli-attach {job}\nStop and delete scratch disk: cargo run -- infra cli-stop {job}"
            );
            io::stdout().flush()?;
        }
        let volumes = config.get("volume_configurations").map(Value::to_string);
        let mut volume_args = Vec::new();
        if let Some(volumes) = &volumes {
            volume_args.extend(["--volume-configurations", volumes.as_str()]);
        }
        launch_attempted = true;
        let result = aws
            .call(
                &[
                    &[
                        "ecs",
                        "run-task",
                        "--cluster",
                        cluster,
                        "--task-definition",
                        revision.as_deref().context("missing task revision")?,
                        "--launch-type",
                        "FARGATE",
                        "--platform-version",
                        "1.4.0",
                        "--count",
                        "1",
                        "--client-token",
                        &job,
                        "--started-by",
                        &job,
                        "--enable-execute-command",
                        "--network-configuration",
                        &network,
                    ][..],
                    volume_args.as_slice(),
                ]
                .concat(),
            )
            .with_context(|| {
                format!("launch outcome uncertain; check ECS tasks started by {job}")
            })?;
        if result["tasks"].as_array().is_some_and(Vec::is_empty) {
            launch_attempted = false;
            bail!("Fargate did not start a task; inspect ECS failures");
        }
        task = Some(text(&result["tasks"][0], "taskArn")?.to_owned());
        let task = task.as_deref().context("missing task ARN")?;
        println!("Task: {task}\nImage: {chosen}");
        let indicator = StartupIndicator::start();
        let deadline = Instant::now() + timeout;
        loop {
            interrupt()?;
            let response = aws.call(&[
                "ecs",
                "describe-tasks",
                "--cluster",
                cluster,
                "--tasks",
                task,
            ])?;
            if response["failures"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
                || response["tasks"][0].is_null()
            {
                bail!("unable to inspect task {task}");
            }
            let status = &response["tasks"][0];
            indicator.update(status["lastStatus"].as_str().unwrap_or("UNKNOWN"));
            if status["lastStatus"] == "STOPPED" {
                bail!("shell task stopped before connecting; inspect ECS and CloudWatch");
            }
            let ready = status["containers"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .filter(|item| item["name"] == "cli")
                    .any(|item| {
                        item["managedAgents"].as_array().is_some_and(|agents| {
                            agents.iter().any(|agent| {
                                agent["name"] == "ExecuteCommandAgent"
                                    && agent["lastStatus"] == "RUNNING"
                            })
                        })
                    })
            });
            if status["lastStatus"] == "RUNNING" && ready {
                break;
            }
            if Instant::now() >= deadline {
                bail!("timed out waiting for the ECS Exec agent");
            }
            std::thread::sleep(Duration::from_secs(5));
        }
        drop(indicator);
        interrupt()?;
        retain = matches!(mode, SessionMode::Persistent { .. });
        session(task)
    })();
    // Attempt both cleanup steps even if one fails. Do not deregister after an
    // uncertain launch: the task can still reference this revision until expiry.
    // Once ready, persistent tasks survive even a failed initial Exec connection.
    if retain {
        println!("Persistent session {job} remains available until expiry or cli-stop.");
    }
    let stop = if let Some(task) = &task
        && !retain
    {
        aws.call(&[
            "ecs",
            "stop-task",
            "--cluster",
            cluster,
            "--task",
            task,
            "--reason",
            "Interactive shell session ended",
        ])
        .map(|_| ())
    } else {
        Ok(())
    };
    let deregister = if let Some(revision) = &revision {
        if task.is_some() || !launch_attempted {
            aws.call(&[
                "ecs",
                "deregister-task-definition",
                "--task-definition",
                revision,
            ])
            .map(|_| ())
        } else {
            Ok(())
        }
    } else {
        Ok(())
    };
    if let Err(error) = &stop {
        eprintln!("Task cleanup failed: {error:#}");
    }
    if let Err(error) = &deregister {
        eprintln!("Task revision cleanup failed: {error:#}");
    }
    outcome.and(stop).and(deregister)
}

#[cfg(test)]
#[path = "../../tests/unit/infra/shell/tests.rs"]
mod tests;
