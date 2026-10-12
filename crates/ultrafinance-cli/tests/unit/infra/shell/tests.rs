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
                "containerDefinitions":[{"name":"cli","secrets":[{"name":"ULTRAFINANCE_ADMIN_USERNAME","valueFrom":"secret:username::"},{"name":"ULTRAFINANCE_ADMIN_PASSWORD","valueFrom":"secret:password::"}]}]}})
            }
            "get-function" => json!(image()),
            "list-versions-by-function" => json!(["9", "$LATEST", "11", "10"]),
            "register-task-definition" => {
                let request: Value =
                    serde_json::from_slice(&std::fs::read(args[3].trim_start_matches("file://"))?)?;
                assert!(request.get("revision").is_none());
                assert_eq!(
                    request["containerDefinitions"][0]["secrets"][0]["valueFrom"],
                    "secret:username::"
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
fn administrator_bootstrap_encodes_secrets_without_printing_them() {
    let script = include_str!("../../../../src/infra/admin-shell.sh");
    let verify = r#"[[ $ULTRAFINANCE_DATABASE_URL == 'postgresql://admin%20%40user:p%40ss%3A%2F%3F%23%25%26%2B%C3%A9@database.example:5432/finance%20db?sslmode=require' ]] || exit 1
[[ -z ${ULTRAFINANCE_ADMIN_PASSWORD+x} ]] || exit 2
printf 'administrator URL validated\n'"#;
    let result = std::process::Command::new("/bin/bash")
        .args(["-c", script, "admin-shell", "/bin/bash", "-c", verify])
        .env("ULTRAFINANCE_ADMIN_USERNAME", "admin @user")
        .env("ULTRAFINANCE_ADMIN_PASSWORD", "p@ss:/?#%&+é")
        .env("ULTRAFINANCE_DATABASE_HOST", "database.example")
        .env("ULTRAFINANCE_DATABASE_NAME", "finance db")
        .env(
            "ULTRAFINANCE_DATABASE_URL",
            "postgresql://old-application-login",
        )
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"administrator URL validated\n");
    assert!(result.stderr.is_empty());
    let missing = std::process::Command::new("/bin/bash")
        .args(["-c", script])
        .env("ULTRAFINANCE_ADMIN_USERNAME", "admin")
        .env_remove("ULTRAFINANCE_ADMIN_PASSWORD")
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("administrator password missing"));
    assert!(missing.stdout.is_empty());
}

#[test]
fn application_credentials_cannot_launch_an_administrator_shell() {
    let definition = json!({"containerDefinitions":[{"name":"cli","secrets":[{"name":"ULTRAFINANCE_DATABASE_URL","valueFrom":"application-secret"}]}]});
    assert!(
        registration(&definition, &image())
            .unwrap_err()
            .to_string()
            .contains("administrator credentials missing")
    );
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
