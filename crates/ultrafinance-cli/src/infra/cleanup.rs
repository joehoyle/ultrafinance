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
#[path = "../../tests/unit/infra/cleanup/tests.rs"]
mod tests;
