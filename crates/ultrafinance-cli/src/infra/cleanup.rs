use super::aws::{Api, Aws, text};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::Path;

pub fn run(root: &Path) -> Result<()> {
    let config = super::shell::config(root)?;
    let aws = Aws::configured(root)?;
    let count = cleanup(&aws, &config)?;
    println!("Stopped {count} CLI task(s).");
    Ok(())
}

fn cleanup(aws: &impl Api, config: &Value) -> Result<usize> {
    let cluster = text(config, "cluster")?;
    let definition = aws.call(&[
        "ecs",
        "describe-task-definition",
        "--task-definition",
        text(config, "task_definition")?,
    ])?;
    let family = text(&definition["taskDefinition"], "family")?;
    // AWS CLI automatically collects every page. Desired RUNNING includes
    // tasks still starting, so an interrupted launch is covered as well.
    let response = aws.call(&[
        "ecs",
        "list-tasks",
        "--cluster",
        cluster,
        "--family",
        family,
        "--launch-type",
        "FARGATE",
        "--desired-status",
        "RUNNING",
    ])?;
    let tasks: Vec<&str> = response["taskArns"]
        .as_array()
        .context("AWS response missing taskArns")?
        .iter()
        .map(|task| task.as_str().context("invalid task ARN"))
        .collect::<Result<_>>()?;
    let mut stopped = Vec::new();
    let mut failed = false;
    for task in &tasks {
        match aws.call(&[
            "ecs",
            "stop-task",
            "--cluster",
            cluster,
            "--task",
            task,
            "--reason",
            "Explicit CLI cleanup",
        ]) {
            Ok(_) => {
                println!("Stopping CLI task: {task}");
                stopped.push(*task);
            }
            Err(error) => {
                eprintln!("Could not stop CLI task {task}: {error:#}");
                failed = true;
            }
        }
    }
    // ECS accepts at most 100 task identifiers per request.
    for batch in stopped.chunks(100) {
        let mut args = vec![
            "ecs",
            "wait",
            "tasks-stopped",
            "--cluster",
            cluster,
            "--tasks",
        ];
        args.extend_from_slice(batch);
        if let Err(error) = aws.call(&args) {
            eprintln!("Could not verify CLI tasks stopped: {error:#}");
            failed = true;
        }
    }
    if failed {
        bail!("CLI cleanup incomplete; rerun infra cli-cleanup to retry");
    }
    Ok(stopped.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;

    struct Fake {
        tasks: Vec<String>,
        fail_stop: bool,
        fail_wait: bool,
        calls: RefCell<Vec<Vec<String>>>,
    }

    impl Api for Fake {
        fn call(&self, args: &[&str]) -> Result<Value> {
            self.calls
                .borrow_mut()
                .push(args.iter().map(|s| s.to_string()).collect());
            match args[1] {
                "describe-task-definition" => Ok(json!({"taskDefinition":{"family":"app-cli"}})),
                "list-tasks" => {
                    assert_eq!(
                        args,
                        [
                            "ecs",
                            "list-tasks",
                            "--cluster",
                            "cluster",
                            "--family",
                            "app-cli",
                            "--launch-type",
                            "FARGATE",
                            "--desired-status",
                            "RUNNING"
                        ]
                    );
                    Ok(json!({"taskArns":self.tasks}))
                }
                "stop-task" if self.fail_stop && args[5] == "task-0" => bail!("stop failed"),
                "wait" if self.fail_wait => bail!("wait failed"),
                _ => Ok(Value::Null),
            }
        }
    }

    fn fake(count: usize) -> Fake {
        Fake {
            tasks: (0..count).map(|i| format!("task-{i}")).collect(),
            fail_stop: false,
            fail_wait: false,
            calls: RefCell::default(),
        }
    }

    fn config() -> Value {
        json!({"cluster":"cluster","task_definition":"definition"})
    }

    #[test]
    fn empty_cluster_is_a_noop() {
        let aws = fake(0);
        assert_eq!(cleanup(&aws, &config()).unwrap(), 0);
        assert_eq!(aws.calls.borrow().len(), 2);
    }

    #[test]
    fn stops_all_tasks_and_waits_in_batches() {
        let aws = fake(101);
        assert_eq!(cleanup(&aws, &config()).unwrap(), 101);
        let calls = aws.calls.borrow();
        assert_eq!(
            calls.iter().filter(|call| call[1] == "stop-task").count(),
            101
        );
        let waits: Vec<_> = calls.iter().filter(|call| call[1] == "wait").collect();
        assert_eq!(waits.len(), 2);
        assert_eq!(waits[0].len(), 106);
        assert_eq!(waits[1].len(), 7);
    }

    #[test]
    fn stop_failure_still_cleans_up_other_tasks() {
        let mut aws = fake(2);
        aws.fail_stop = true;
        assert!(cleanup(&aws, &config()).is_err());
        let calls = aws.calls.borrow();
        assert_eq!(
            calls.iter().filter(|call| call[1] == "stop-task").count(),
            2
        );
        assert_eq!(calls.last().unwrap().last().unwrap(), "task-1");
    }

    #[test]
    fn wait_failure_is_reported() {
        let mut aws = fake(1);
        aws.fail_wait = true;
        assert!(cleanup(&aws, &config()).is_err());
    }
}
