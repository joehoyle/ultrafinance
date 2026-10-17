use super::*;
use serde_json::json;
use std::cell::RefCell;

const ID: &str = "1234567890abcdef1234567890abcdef";

struct Fake {
    calls: RefCell<Vec<Vec<String>>>,
    persistent: bool,
    family: &'static str,
    status: &'static str,
    missing: bool,
    started_by: &'static str,
    stop_fails: bool,
    snapshot_state: &'static str,
    snapshot_fails: bool,
    attached: bool,
}

impl Api for Fake {
    fn call(&self, args: &[&str]) -> Result<Value> {
        self.calls
            .borrow_mut()
            .push(args.iter().map(|value| value.to_string()).collect());
        Ok(match args[1] {
            "list-tasks" => {
                assert_eq!(&args[4..], &["--started-by", ID]);
                json!({"taskArns":if self.missing {vec![]} else {vec!["task"]}})
            }
            "describe-tasks" => json!({"tasks":[{
                "taskArn":"task","taskDefinitionArn":"session-definition",
                "startedBy":self.started_by,"lastStatus":self.status,"desiredStatus":"RUNNING",
                "attachments": if self.attached {json!([{"type":"AmazonElasticBlockStorage","status":"ATTACHED","details":[
                    {"name":"volumeName","value":"scratch"},{"name":"volumeId","value":"vol-123"}
                ]}])} else {json!([])}
            }]}),
            "describe-task-definition" => {
                if args[3] == "configured-definition" {
                    json!({"taskDefinition":{"family":"cli"}})
                } else {
                    json!({"taskDefinition":{"family":self.family,"containerDefinitions":[{
                        "name":"cli","environment":[{"name":"ULTRAFINANCE_CLI_PERSISTENT","value":if self.persistent {"1"} else {"0"}}]
                    }]}})
                }
            }
            "create-snapshot" if self.snapshot_fails => bail!("snapshot failed"),
            "create-snapshot" => {
                assert_eq!(args[3], "vol-123");
                json!({"SnapshotId":"snap-0123456789abcdef0"})
            }
            "describe-snapshots" => {
                json!({"Snapshots":[{"SnapshotId":"snap-0123456789abcdef0","State":self.snapshot_state,"VolumeSize":2048,"VolumeId":"vol-123","Progress":"50%"}]})
            }
            "stop-task" if self.stop_fails => bail!("stop failed"),
            _ => json!({}),
        })
    }
}

fn fake() -> Fake {
    Fake {
        calls: RefCell::new(vec![]),
        persistent: true,
        family: "cli",
        status: "RUNNING",
        missing: false,
        started_by: ID,
        stop_fails: false,
        snapshot_state: "completed",
        snapshot_fails: false,
        attached: true,
    }
}
fn config() -> Value {
    json!({"cluster":"cluster","task_definition":"configured-definition"})
}

#[test]
fn reconnect_uses_existing_task_and_never_launches_or_stops_even_when_exec_fails() {
    for fail in [false, true] {
        let aws = fake();
        let result = attach_session(&aws, &config(), ID, |task| {
            assert_eq!(task, "task");
            if fail {
                bail!("connection lost")
            } else {
                Ok(())
            }
        });
        assert_eq!(result.is_err(), fail);
        assert!(!aws.calls.borrow().iter().any(|call| matches!(
            call[1].as_str(),
            "run-task" | "stop-task" | "deregister-task-definition"
        )));
    }
}

#[test]
fn explicit_stop_waits_then_deregisters_only_the_session_revision() {
    let aws = fake();
    stop_session(&aws, &config(), ID, false, None, 6).unwrap();
    let calls = aws.calls.borrow();
    assert_eq!(calls[calls.len() - 3][1], "stop-task");
    assert_eq!(calls[calls.len() - 2][1], "wait");
    assert_eq!(
        calls.last().unwrap(),
        &vec![
            "ecs".to_owned(),
            "deregister-task-definition".into(),
            "--task-definition".into(),
            "session-definition".into()
        ]
    );
}

#[test]
fn malformed_missing_foreign_and_temporary_sessions_are_never_stopped() {
    let aws = fake();
    assert!(stop_session(&aws, &config(), "not-a-session", false, None, 6).is_err());
    assert!(aws.calls.borrow().is_empty());
    for case in 0..4 {
        let mut aws = fake();
        match case {
            0 => aws.missing = true,
            1 => aws.persistent = false,
            2 => aws.family = "another-app",
            _ => aws.started_by = "another-session",
        }
        assert!(stop_session(&aws, &config(), ID, false, None, 6).is_err());
        assert!(!aws.calls.borrow().iter().any(|call| call[1] == "stop-task"));
    }
}

#[test]
fn stopped_tasks_cannot_be_attached_and_failed_stop_is_not_reported_as_success() {
    let mut aws = fake();
    aws.status = "STOPPED";
    assert!(attach_session(&aws, &config(), ID, |_| panic!("must not attach")).is_err());
    let mut aws = fake();
    aws.stop_fails = true;
    assert!(stop_session(&aws, &config(), ID, false, None, 6).is_err());
    assert!(
        !aws.calls
            .borrow()
            .iter()
            .any(|call| call[1] == "wait" || call[1] == "deregister-task-definition")
    );
}

#[test]
fn saving_snapshot_completes_before_stop_and_failures_never_stop_task() {
    let aws = fake();
    stop_session(&aws, &config(), ID, true, None, 6).unwrap();
    let calls = aws.calls.borrow();
    let saved = calls
        .iter()
        .position(|c| c[1] == "describe-snapshots")
        .unwrap();
    let stopped = calls.iter().position(|c| c[1] == "stop-task").unwrap();
    assert!(saved < stopped);
    drop(calls);
    for case in 0..4 {
        let mut aws = fake();
        match case {
            0 => aws.snapshot_fails = true,
            1 => aws.snapshot_state = "error",
            2 => aws.attached = false,
            _ => aws.status = "STOPPED",
        }
        assert!(stop_session(&aws, &config(), ID, true, None, 6).is_err());
        assert!(
            !aws.calls
                .borrow()
                .iter()
                .any(|c| matches!(c[1].as_str(), "stop-task" | "deregister-task-definition"))
        );
    }
}

#[test]
fn pending_snapshot_timeout_and_interruption_leave_task_running() {
    for interrupt in [false, true] {
        let mut aws = fake();
        aws.snapshot_state = "pending";
        let task = resolve(&aws, &config(), ID).unwrap();
        let result = save_snapshot(
            &aws,
            &task,
            ID,
            None,
            if interrupt {
                Duration::from_secs(60)
            } else {
                Duration::ZERO
            },
            || bail!("interrupted"),
        );
        let error = result.unwrap_err().to_string();
        assert!(error.contains("snap-0123456789abcdef0") && error.contains("not stopped"));
        assert!(!aws.calls.borrow().iter().any(|c| c[1] == "stop-task"));
    }
}

fn volume_config() -> Value {
    json!({"volume_configurations":[{"name":"scratch","managedEBSVolume":{
        "sizeInGiB":1024,"encrypted":true,"filesystemType":"ext4",
        "roleArn":"volume-role","terminationPolicy":{"deleteOnTermination":true}
    }}]})
}

#[test]
fn restore_requires_completed_snapshot_and_keeps_volume_configuration() {
    let aws = fake();
    let mut config = volume_config();
    restore_snapshot(&aws, &mut config, "snap-0123456789abcdef0").unwrap();
    let volume = &config["volume_configurations"][0]["managedEBSVolume"];
    assert_eq!(volume["snapshotId"], "snap-0123456789abcdef0");
    assert_eq!(volume["sizeInGiB"], 2048);
    assert_eq!(volume["roleArn"], "volume-role");
    assert_eq!(volume["encrypted"], true);
    assert_eq!(volume["terminationPolicy"]["deleteOnTermination"], true);
    config["volume_configurations"][0]["managedEBSVolume"]["sizeInGiB"] = json!(4096);
    restore_snapshot(&aws, &mut config, "snap-0123456789abcdef0").unwrap();
    assert_eq!(
        config["volume_configurations"][0]["managedEBSVolume"]["sizeInGiB"],
        4096
    );
    for state in ["pending", "error"] {
        let mut aws = fake();
        aws.snapshot_state = state;
        let mut config = volume_config();
        let original = config.clone();
        assert!(restore_snapshot(&aws, &mut config, "snap-0123456789abcdef0").is_err());
        assert_eq!(config, original);
        assert!(!aws.calls.borrow().iter().any(|c| c[1] == "run-task"));
    }
}

#[test]
fn bad_snapshot_ids_and_missing_volume_configuration_fail_before_aws_calls() {
    let aws = fake();
    for id in [
        "",
        "vol-0123456789abcdef0",
        "snap-INVALID",
        "snap-0123456789abcdef0 --other",
    ] {
        assert!(restore_snapshot(&aws, &mut volume_config(), id).is_err());
    }
    assert!(restore_snapshot(&aws, &mut config(), "snap-0123456789abcdef0").is_err());
    assert!(aws.calls.borrow().is_empty());
}

#[test]
fn saving_waits_through_pending_progress_until_completed() {
    struct Pending {
        aws: Fake,
        polls: std::cell::Cell<usize>,
    }
    impl Api for Pending {
        fn call(&self, args: &[&str]) -> Result<Value> {
            let mut value = self.aws.call(args)?;
            if args[1] == "describe-snapshots" {
                self.polls.set(self.polls.get() + 1);
                if self.polls.get() < 3 {
                    value["Snapshots"][0]["State"] = json!("pending");
                }
            }
            Ok(value)
        }
    }
    let aws = Pending {
        aws: fake(),
        polls: std::cell::Cell::new(0),
    };
    let task = resolve(&aws, &config(), ID).unwrap();
    let waits = std::cell::Cell::new(0);
    let id = save_snapshot(&aws, &task, ID, None, Duration::from_secs(60), || {
        waits.set(waits.get() + 1);
        Ok(())
    })
    .unwrap();
    assert_eq!(id, "snap-0123456789abcdef0");
    assert_eq!(waits.get(), 2);
    assert_eq!(aws.polls.get(), 3);
}

#[test]
fn existing_snapshot_is_reused_without_creating_another_before_stop() {
    let aws = fake();
    stop_session(&aws, &config(), ID, true, Some("snap-0123456789abcdef0"), 6).unwrap();
    let calls = aws.calls.borrow();
    assert!(!calls.iter().any(|c| c[1] == "create-snapshot"));
    let checked = calls
        .iter()
        .position(|c| c[1] == "describe-snapshots")
        .unwrap();
    let stopped = calls.iter().position(|c| c[1] == "stop-task").unwrap();
    assert!(checked < stopped);
}

#[test]
fn unrelated_snapshot_never_stops_a_session() {
    struct OtherVolume(Fake);
    impl Api for OtherVolume {
        fn call(&self, args: &[&str]) -> Result<Value> {
            let mut response = self.0.call(args)?;
            if args[1] == "describe-snapshots" {
                response["Snapshots"][0]["VolumeId"] = json!("vol-other");
            }
            Ok(response)
        }
    }
    let aws = OtherVolume(fake());
    assert!(stop_session(&aws, &config(), ID, true, Some("snap-0123456789abcdef0"), 6).is_err());
    assert!(
        !aws.0
            .calls
            .borrow()
            .iter()
            .any(|c| matches!(c[1].as_str(), "create-snapshot" | "stop-task"))
    );
}

#[test]
fn invalid_wait_and_snapshot_without_save_fail_before_aws_calls() {
    let aws = fake();
    for hours in [0, 25] {
        assert!(stop_session(&aws, &config(), ID, true, None, hours).is_err());
    }
    assert!(
        stop_session(
            &aws,
            &config(),
            ID,
            false,
            Some("snap-0123456789abcdef0"),
            6
        )
        .is_err()
    );
    assert!(aws.calls.borrow().is_empty());
}
