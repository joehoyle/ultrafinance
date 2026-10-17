use super::{
    aws::{Api, Aws, text},
    shell,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    io::{self, Write},
    path::Path,
    time::{Duration, Instant},
};

fn validate_snapshot(id: &str) -> Result<()> {
    let suffix = id
        .strip_prefix("snap-")
        .context("use an EBS snapshot ID (snap-...)")?;
    if !matches!(suffix.len(), 8 | 17)
        || !suffix
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        bail!("use an EBS snapshot ID (snap-...)");
    }
    Ok(())
}

fn describe_snapshot(aws: &impl Api, id: &str) -> Result<Value> {
    let response = aws.call(&["ec2", "describe-snapshots", "--snapshot-ids", id])?;
    let snapshots = response["Snapshots"]
        .as_array()
        .context("snapshot response missing Snapshots")?;
    if snapshots.len() != 1 || snapshots[0]["SnapshotId"] != id {
        bail!("snapshot response does not match {id}");
    }
    Ok(snapshots[0].clone())
}

pub(super) fn restore_snapshot(aws: &impl Api, config: &mut Value, id: &str) -> Result<()> {
    validate_snapshot(id)?;
    let volume = config["volume_configurations"]
        .as_array_mut()
        .and_then(|volumes| {
            volumes
                .iter_mut()
                .find(|volume| volume["name"] == "scratch")
        })
        .and_then(|volume| volume["managedEBSVolume"].as_object_mut())
        .context("CLI infrastructure has no managed scratch volume")?;
    let snapshot = describe_snapshot(aws, id)?;
    if snapshot["State"] != "completed" {
        bail!("snapshot {id} is not completed; no task was launched");
    }
    let size = snapshot["VolumeSize"]
        .as_u64()
        .filter(|size| *size > 0 && *size <= 16384)
        .context("snapshot has invalid volume size")?;
    let configured = volume
        .get("sizeInGiB")
        .and_then(Value::as_u64)
        .context("scratch size missing")?;
    volume.insert("snapshotId".into(), json!(id));
    volume.insert("sizeInGiB".into(), json!(configured.max(size)));
    println!(
        "Restoring /scratch from snapshot {id} ({} GiB disk).",
        configured.max(size)
    );
    Ok(())
}

fn save_snapshot(
    aws: &impl Api,
    task: &Value,
    id: &str,
    existing: Option<&str>,
    timeout: Duration,
    wait: impl Fn() -> Result<()>,
) -> Result<String> {
    if task["lastStatus"] != "RUNNING" || task["desiredStatus"] != "RUNNING" {
        bail!("session must be running to save its scratch data");
    }
    let attachments = task["attachments"]
        .as_array()
        .context("task has no volume attachments")?;
    let matches: Vec<_> = attachments
        .iter()
        .filter(|attachment| {
            attachment["type"] == "AmazonElasticBlockStorage"
                && attachment["status"] == "ATTACHED"
                && attachment["details"].as_array().is_some_and(|details| {
                    details.iter().any(|detail| {
                        detail["name"] == "volumeName" && detail["value"] == "scratch"
                    })
                })
        })
        .collect();
    if matches.len() != 1 {
        bail!("task has no unique attached scratch disk; session was not stopped");
    }
    let volume = matches[0]["details"]
        .as_array()
        .unwrap()
        .iter()
        .find(|detail| detail["name"] == "volumeId")
        .context("scratch attachment missing volumeId")?;
    let tags = json!([{"ResourceType":"snapshot","Tags":[
        {"Key":"Name","Value":format!("ultrafinance-cli-{id}")},
        {"Key":"ultrafinance:cli-session","Value":id}
    ]}])
    .to_string();
    println!(
        "Saving scratch disk. Finish any writes and run sync in the session before saving; running processes are not preserved."
    );
    let snapshot = if let Some(existing) = existing {
        validate_snapshot(existing)?;
        let current = describe_snapshot(aws, existing)?;
        if current["VolumeId"] != text(volume, "value")? {
            bail!(
                "snapshot {existing} is not from this session's scratch disk; session was not stopped"
            );
        }
        println!(
            "Reusing snapshot {existing}. Only files captured when it was created are saved; subsequent writes are not included."
        );
        existing.to_owned()
    } else {
        let created = aws.call(&[
            "ec2",
            "create-snapshot",
            "--volume-id",
            text(volume, "value")?,
            "--description",
            &format!("Ultrafinance CLI scratch session {id}"),
            "--tag-specifications",
            &tags,
        ])?;
        text(&created, "SnapshotId")?.to_owned()
    };
    validate_snapshot(&snapshot)?;
    println!(
        "Snapshot: {snapshot}\nWaiting for snapshot completion before stopping session {id}..."
    );
    io::stdout().flush()?;
    let started = Instant::now();
    let mut last_progress = None;
    loop {
        let current = describe_snapshot(aws, &snapshot)?;
        match current["State"].as_str() {
            Some("completed") => break,
            Some("pending") => {}
            _ => bail!("snapshot {snapshot} did not complete; session was not stopped"),
        }
        let progress = current["Progress"].as_str().unwrap_or("pending");
        if last_progress.as_deref() != Some(progress) {
            println!("Snapshot {snapshot}: {progress}");
            last_progress = Some(progress.to_owned());
        }
        if started.elapsed() >= timeout {
            bail!(
                "timed out waiting for snapshot {snapshot}; session was not stopped. Resume without creating another snapshot: cargo run -- infra cli-stop {id} --save-data --snapshot {snapshot} --wait-hours 6"
            );
        }
        wait().with_context(|| {
            format!("snapshot {snapshot} may still finish; session was not stopped")
        })?;
    }
    println!("Snapshot saved. Restore: cargo run -- infra cli --persistent --snapshot {snapshot}");
    io::stdout().flush()?;
    Ok(snapshot)
}

fn resolve(aws: &impl Api, config: &Value, id: &str) -> Result<Value> {
    if id.len() != 32
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("use the 32-character session ID printed by infra cli --persistent");
    }
    let response = aws.call(&[
        "ecs",
        "list-tasks",
        "--cluster",
        text(config, "cluster")?,
        "--started-by",
        id,
    ])?;
    let tasks = response["taskArns"]
        .as_array()
        .context("AWS response missing taskArns")?;
    if tasks.len() != 1 {
        bail!("no unique active CLI session {id}; it may have expired or been stopped");
    }
    let arn = tasks[0].as_str().context("invalid task ARN")?;
    let response = aws.call(&[
        "ecs",
        "describe-tasks",
        "--cluster",
        text(config, "cluster")?,
        "--tasks",
        arn,
    ])?;
    let task = response["tasks"]
        .as_array()
        .and_then(|tasks| tasks.first())
        .context("session task not found")?;
    if task["startedBy"] != id || task["taskArn"] != arn {
        bail!("task does not match this session");
    }
    let definition = aws.call(&[
        "ecs",
        "describe-task-definition",
        "--task-definition",
        text(task, "taskDefinitionArn")?,
    ])?;
    let configured = aws.call(&[
        "ecs",
        "describe-task-definition",
        "--task-definition",
        text(config, "task_definition")?,
    ])?;
    let persistent = definition["taskDefinition"]["containerDefinitions"]
        .as_array()
        .is_some_and(|containers| {
            containers.iter().any(|container| {
                container["name"] == "cli"
                    && container["environment"]
                        .as_array()
                        .is_some_and(|environment| {
                            environment.iter().any(|value| {
                                value["name"] == "ULTRAFINANCE_CLI_PERSISTENT"
                                    && value["value"] == "1"
                            })
                        })
            })
        });
    if !persistent
        || text(&definition["taskDefinition"], "family")?
            != text(&configured["taskDefinition"], "family")?
    {
        bail!("session is not a persistent task from this CLI family");
    }
    Ok(task.clone())
}

pub(super) fn attach(root: &Path, id: &str) -> Result<()> {
    shell::preflight()?;
    let config = shell::config(root)?;
    let aws = Aws::configured(root)?;
    attach_session(&aws, &config, id, |task| {
        shell::execute(&aws, &config, task, true)
    })
}

fn attach_session(
    aws: &impl Api,
    config: &Value,
    id: &str,
    session: impl FnOnce(&str) -> Result<()>,
) -> Result<()> {
    let task = resolve(aws, config, id)?;
    if task["lastStatus"] != "RUNNING" || task["desiredStatus"] != "RUNNING" {
        bail!("session is not running; it may be starting, stopping, or expired");
    }
    println!(
        "Reconnecting to {id}. Detach with Ctrl-B then D; disconnecting keeps the task until expiry or cli-stop."
    );
    session(text(&task, "taskArn")?)
}

pub(super) fn stop(
    root: &Path,
    id: &str,
    save_data: bool,
    snapshot: Option<&str>,
    wait_hours: u32,
) -> Result<()> {
    let config = shell::config(root)?;
    let aws = Aws::configured(root)?;
    stop_session(&aws, &config, id, save_data, snapshot, wait_hours)
}

fn stop_session(
    aws: &impl Api,
    config: &Value,
    id: &str,
    save_data: bool,
    snapshot: Option<&str>,
    wait_hours: u32,
) -> Result<()> {
    if !(1..=24).contains(&wait_hours) || (snapshot.is_some() && !save_data) {
        bail!("snapshot reuse requires save-data; snapshot wait must be 1–24 hours");
    }
    let task = resolve(aws, config, id)?;
    if save_data {
        let interrupts = shell::Interrupts::install()?;
        save_snapshot(
            aws,
            &task,
            id,
            snapshot,
            Duration::from_secs(u64::from(wait_hours) * 3600),
            || {
                interrupts.check()?;
                std::thread::sleep(Duration::from_secs(5));
                interrupts.check()
            },
        )
        .context("could not save scratch data; no stop request was sent")?;
        interrupts.check()?;
    }
    let cluster = text(config, "cluster")?;
    let arn = text(&task, "taskArn")?;
    aws.call(&[
        "ecs",
        "stop-task",
        "--cluster",
        cluster,
        "--task",
        arn,
        "--reason",
        "Explicit persistent CLI stop",
    ])?;
    aws.call(&[
        "ecs",
        "wait",
        "tasks-stopped",
        "--cluster",
        cluster,
        "--tasks",
        arn,
    ])?;
    let revision = text(&task, "taskDefinitionArn")?;
    if revision != text(config, "task_definition")? {
        aws.call(&[
            "ecs",
            "deregister-task-definition",
            "--task-definition",
            revision,
        ])?;
    }
    println!(
        "Stopped session {id}. Its scratch disk is configured for deletion on task termination."
    );
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/infra/persistent/tests.rs"]
mod tests;
