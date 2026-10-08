use super::aws::{Api, Aws, text, validate_image};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    io::{self, IsTerminal, Write},
    path::Path,
    time::{Duration, Instant},
};

#[cfg(unix)]
static INTERRUPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(unix)]
extern "C" fn on_interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, std::sync::atomic::Ordering::Relaxed);
}

struct Interrupts {
    #[cfg(unix)]
    previous: libc::sigaction,
}

impl Interrupts {
    fn install() -> Result<Self> {
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

    fn check(&self) -> Result<()> {
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

pub fn open(root: &Path, image: ImageSelection<'_>) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("run infra cli in an interactive terminal");
    }
    if !std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .any(|path| path.join("session-manager-plugin").is_file())
    {
        bail!("install the AWS Session Manager plugin before opening a shell");
    }
    let config = config(root)?;
    let aws = Aws::configured(root)?;
    let interrupts = Interrupts::install()?;
    run_shell(
        &aws,
        &config,
        image,
        Duration::from_secs(300),
        || interrupts.check(),
        |task| {
            println!(
                "Opening shell. Ctrl-C cancels remote commands; use exit to stop the task. The task expires after one hour."
            );
            io::stdout().flush()?;
            super::checked(aws.command().args([
                "ecs",
                "execute-command",
                "--cluster",
                text(&config, "cluster")?,
                "--task",
                task,
                "--container",
                "cli",
                "--interactive",
                "--command",
                "/bin/bash --rcfile /etc/bash.bashrc -i",
            ]))
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

fn run_shell(
    aws: &impl Api,
    config: &Value,
    image: ImageSelection<'_>,
    timeout: Duration,
    interrupt: impl Fn() -> Result<()>,
    session: impl FnOnce(&str) -> Result<()>,
) -> Result<()> {
    let cluster = text(config, "cluster")?;
    let job = uuid::Uuid::new_v4().simple().to_string();
    let mut task: Option<String> = None;
    let mut revision: Option<String> = None;
    let mut launch_attempted = false;
    let outcome = (|| -> Result<()> {
        interrupt()?;
        let definition = aws.call(&[
            "ecs",
            "describe-task-definition",
            "--task-definition",
            text(config, "task_definition")?,
        ])?;
        let chosen = resolve_image(aws, text(config, "function_name")?, image)?;
        let request = registration(&definition["taskDefinition"], &chosen)?;
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
        launch_attempted = true;
        let result = aws
            .call(&[
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
            ])
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
        let has_database = request["containerDefinitions"]
            .as_array()
            .is_some_and(|items| {
                items
                    .iter()
                    .filter(|item| item["name"] == "cli")
                    .any(|item| {
                        item["secrets"].as_array().is_some_and(|secrets| {
                            secrets
                                .iter()
                                .any(|secret| secret["name"] == "ULTRAFINANCE_DATABASE_URL")
                        })
                    })
            });
        if !has_database {
            println!("Database URL is not configured; set database_url in infrastructure.");
        }
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
        interrupt()?;
        session(task)
    })();
    // Attempt both cleanup steps even if one fails. Do not deregister after an
    // uncertain launch: the task can still reference this revision for an hour.
    let stop = if let Some(task) = &task {
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
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn image() -> String {
        format!(
            "123456789012.dkr.ecr.ca-central-1.amazonaws.com/ultrafinance@sha256:{}",
            "a".repeat(64)
        )
    }

    struct Fake {
        calls: RefCell<Vec<Vec<String>>>,
        launch_error: bool,
        rejected: bool,
        ready: bool,
    }

    impl Api for Fake {
        fn call(&self, args: &[&str]) -> Result<Value> {
            self.calls
                .borrow_mut()
                .push(args.iter().map(|s| s.to_string()).collect());
            Ok(match args[1] {
                "describe-task-definition" => {
                    json!({"taskDefinition": {"family":"cli", "revision":5,
                    "containerDefinitions":[{"name":"cli","secrets":[{"name":"ULTRAFINANCE_DATABASE_URL","valueFrom":"secret"}]}]}})
                }
                "get-function" => json!(image()),
                "list-versions-by-function" => json!(["9", "$LATEST", "11", "10"]),
                "register-task-definition" => {
                    let request: Value = serde_json::from_slice(&std::fs::read(
                        args[3].trim_start_matches("file://"),
                    )?)?;
                    assert!(request.get("revision").is_none());
                    assert_eq!(
                        request["containerDefinitions"][0]["secrets"][0]["valueFrom"],
                        "secret"
                    );
                    json!({"taskDefinition":{"taskDefinitionArn":"revision"}})
                }
                "run-task" => {
                    if self.launch_error {
                        bail!("uncertain launch");
                    }
                    let network: Value = serde_json::from_str(args.last().unwrap())?;
                    assert_eq!(network["awsvpcConfiguration"]["assignPublicIp"], "DISABLED");
                    assert!(args.contains(&"--enable-execute-command"));
                    if self.rejected {
                        json!({"tasks":[],"failures":[{"reason":"capacity"}]})
                    } else {
                        json!({"tasks":[{"taskArn":"task"}]})
                    }
                }
                "describe-tasks" => {
                    json!({"tasks":[{"lastStatus":"RUNNING","containers":[{"name":"cli",
                    "managedAgents":[{"name":"ExecuteCommandAgent","lastStatus":if self.ready {"RUNNING"} else {"PENDING"}}]}]}]})
                }
                _ => json!({}),
            })
        }
    }

    fn fake() -> Fake {
        Fake {
            calls: RefCell::new(vec![]),
            launch_error: false,
            rejected: false,
            ready: true,
        }
    }

    fn config() -> Value {
        json!({"cluster":"cluster","task_definition":"definition","function_name":"function",
            "subnets":["private"],"security_groups":["database"]})
    }

    #[test]
    fn latest_selects_highest_published_version_and_live_remains_default() {
        for (selection, qualifier) in [
            (ImageSelection::Live, "live"),
            (ImageSelection::Latest, "11"),
        ] {
            let aws = fake();
            run_shell(
                &aws,
                &config(),
                selection,
                Duration::ZERO,
                || Ok(()),
                |_| Ok(()),
            )
            .unwrap();
            let calls = aws.calls.borrow();
            let get = calls.iter().find(|args| args[1] == "get-function").unwrap();
            let index = get.iter().position(|arg| arg == "--qualifier").unwrap();
            assert_eq!(get[index + 1], qualifier);
            assert_eq!(
                calls
                    .iter()
                    .any(|args| args[1] == "list-versions-by-function"),
                qualifier == "11"
            );
        }
        let aws = fake();
        assert_eq!(
            resolve_image(&aws, "function", ImageSelection::Digest(&image())).unwrap(),
            image()
        );
        assert!(aws.calls.borrow().is_empty());
    }

    #[test]
    fn latest_without_a_published_version_creates_no_task() {
        struct Empty;
        impl Api for Empty {
            fn call(&self, args: &[&str]) -> Result<Value> {
                assert_eq!(args[1], "list-versions-by-function");
                Ok(json!(["$LATEST"]))
            }
        }
        assert!(
            resolve_image(&Empty, "function", ImageSelection::Latest)
                .unwrap_err()
                .to_string()
                .contains("no published Lambda versions")
        );
    }

    #[test]
    fn session_exit_or_failure_cleans_up_task_and_revision() {
        for fail in [false, true] {
            let aws = fake();
            let result = run_shell(
                &aws,
                &config(),
                ImageSelection::Live,
                Duration::ZERO,
                || Ok(()),
                |task| {
                    assert_eq!(task, "task");
                    if fail {
                        bail!("session failed");
                    }
                    Ok(())
                },
            );
            assert_eq!(result.is_err(), fail);
            let calls = aws.calls.borrow();
            assert_eq!(calls[calls.len() - 2][1], "stop-task");
            assert_eq!(calls[calls.len() - 1][1], "deregister-task-definition");
        }
    }

    #[test]
    fn timeout_cleans_up_without_starting_session() {
        let mut aws = fake();
        aws.ready = false;
        let result = run_shell(
            &aws,
            &config(),
            ImageSelection::Live,
            Duration::ZERO,
            || Ok(()),
            |_| panic!("must not open shell"),
        );
        assert!(result.unwrap_err().to_string().contains("timed out"));
        assert!(aws.calls.borrow().iter().any(|call| call[1] == "stop-task"));
    }

    #[test]
    fn uncertain_launch_is_never_retried_or_deregistered() {
        let mut aws = fake();
        aws.launch_error = true;
        let result = run_shell(
            &aws,
            &config(),
            ImageSelection::Live,
            Duration::ZERO,
            || Ok(()),
            |_| panic!("must not open shell"),
        );
        assert!(result.unwrap_err().to_string().contains("uncertain"));
        let calls = aws.calls.borrow();
        assert_eq!(calls.iter().filter(|call| call[1] == "run-task").count(), 1);
        assert!(
            !calls
                .iter()
                .any(|call| call[1] == "deregister-task-definition")
        );
    }

    #[test]
    fn rejected_launch_deregisters_revision_and_tags_are_rejected() {
        let mut aws = fake();
        aws.rejected = true;
        assert!(
            run_shell(
                &aws,
                &config(),
                ImageSelection::Live,
                Duration::ZERO,
                || Ok(()),
                |_| Ok(())
            )
            .is_err()
        );
        assert_eq!(
            aws.calls.borrow().last().unwrap()[1],
            "deregister-task-definition"
        );
        assert!(registration(&json!({}), "repo:latest").is_err());
    }

    #[test]
    fn startup_interrupt_cleans_up_known_task() {
        let aws = fake();
        let interrupt = || {
            if aws.calls.borrow().iter().any(|call| call[1] == "run-task") {
                bail!("interrupted");
            }
            Ok(())
        };
        assert!(
            run_shell(
                &aws,
                &config(),
                ImageSelection::Live,
                Duration::ZERO,
                interrupt,
                |_| panic!("must not open shell")
            )
            .is_err()
        );
        assert_eq!(
            aws.calls.borrow().last().unwrap()[1],
            "deregister-task-definition"
        );
    }
}
