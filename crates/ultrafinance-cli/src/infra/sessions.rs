use super::aws::{Api, Aws, text};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    io::{self, Write},
    path::Path,
};

pub(super) fn run(root: &Path) -> Result<()> {
    let config = super::shell::config(root)?;
    let aws = Aws::configured(root)?;
    let sessions = list(&aws, &config)?;
    display(&mut io::stdout().lock(), &sessions)
}

struct Session {
    id: String,
    status: String,
    started: String,
    task: String,
}

fn list(aws: &impl Api, config: &Value) -> Result<Vec<Session>> {
    let cluster = text(config, "cluster")?;
    let definition = aws.call(&[
        "ecs",
        "describe-task-definition",
        "--task-definition",
        text(config, "task_definition")?,
    ])?;
    let family = text(&definition["taskDefinition"], "family")?;
    // AWS CLI automatically paginates. Desired RUNNING also includes starting tasks.
    let response = aws.call(&[
        "ecs",
        "list-tasks",
        "--cluster",
        cluster,
        "--family",
        family,
        "--desired-status",
        "RUNNING",
    ])?;
    let tasks = response["taskArns"]
        .as_array()
        .context("AWS response missing taskArns")?
        .iter()
        .map(|task| task.as_str().context("invalid task ARN"))
        .collect::<Result<Vec<_>>>()?;
    let mut sessions = Vec::new();
    for batch in tasks.chunks(100) {
        let mut args = vec!["ecs", "describe-tasks", "--cluster", cluster, "--tasks"];
        args.extend_from_slice(batch);
        let response = aws.call(&args)?;
        if response["failures"].as_array().is_some_and(|failures| {
            failures
                .iter()
                .any(|failure| failure["reason"] != "MISSING")
        }) {
            bail!("could not describe all CLI sessions; retry cli-list");
        }
        for task in response["tasks"]
            .as_array()
            .context("AWS response missing tasks")?
        {
            // Tasks may finish between listing and describing them.
            if task["lastStatus"] == "STOPPED" {
                continue;
            }
            let arn = text(task, "taskArn")?;
            sessions.push(Session {
                id: task["startedBy"].as_str().unwrap_or("-").to_owned(),
                status: text(task, "lastStatus")?.to_owned(),
                started: task["startedAt"]
                    .as_str()
                    .or_else(|| task["createdAt"].as_str())
                    .unwrap_or("-")
                    .to_owned(),
                task: arn.rsplit('/').next().unwrap_or(arn).to_owned(),
            });
        }
    }
    sessions.sort_by(|a, b| b.started.cmp(&a.started).then_with(|| a.id.cmp(&b.id)));
    Ok(sessions)
}

fn display(output: &mut impl Write, sessions: &[Session]) -> Result<()> {
    if sessions.is_empty() {
        writeln!(output, "No active CLI sessions.")?;
    } else {
        writeln!(
            output,
            "{:<32}  {:<14}  {:<32}  TASK",
            "SESSION ID", "STATUS", "STARTED"
        )?;
        for session in sessions {
            writeln!(
                output,
                "{:<32}  {:<14}  {:<32}  {}",
                session.id, session.status, session.started, session.task
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/infra/sessions/tests.rs"]
mod tests;
