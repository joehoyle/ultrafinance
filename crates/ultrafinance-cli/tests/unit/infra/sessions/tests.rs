use super::*;
use serde_json::json;
use std::cell::RefCell;

struct Fake {
    tasks: Vec<Value>,
    calls: RefCell<Vec<Vec<String>>>,
    failures: Value,
}

impl Api for Fake {
    fn call(&self, args: &[&str]) -> Result<Value> {
        self.calls
            .borrow_mut()
            .push(args.iter().map(|s| s.to_string()).collect());
        Ok(match args[1] {
            "describe-task-definition" => json!({"taskDefinition":{"family":"ultrafinance-cli"}}),
            "list-tasks" => {
                assert_eq!(
                    &args[4..],
                    &[
                        "--family",
                        "ultrafinance-cli",
                        "--desired-status",
                        "RUNNING"
                    ]
                );
                json!({"taskArns":self.tasks.iter().map(|t| &t["taskArn"]).collect::<Vec<_>>()})
            }
            "describe-tasks" => {
                assert!(args.len() - 5 <= 100);
                json!({"tasks":self.tasks.iter().filter(|t| args[5..].contains(&t["taskArn"].as_str().unwrap())).collect::<Vec<_>>(), "failures":self.failures})
            }
            _ => panic!("listing must only read ECS metadata"),
        })
    }
}

fn fake(tasks: Vec<Value>) -> Fake {
    Fake {
        tasks,
        calls: RefCell::new(Vec::new()),
        failures: json!([]),
    }
}

fn config() -> Value {
    json!({"cluster":"cluster", "task_definition":"definition"})
}

#[test]
fn lists_session_ids_newest_first_including_starting_tasks() {
    let aws = fake(vec![
        json!({"taskArn":"arn/task/old", "startedBy":"old-session", "lastStatus":"RUNNING", "startedAt":"2026-10-13T17:18:38+02:00"}),
        json!({"taskArn":"arn/task/new", "startedBy":"new-session", "lastStatus":"PROVISIONING", "createdAt":"2026-10-13T18:44:38+02:00"}),
        json!({"taskArn":"arn/task/stopped", "startedBy":"finished", "lastStatus":"STOPPED"}),
    ]);
    let sessions = list(&aws, &config()).unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].id, "new-session");
    assert_eq!(sessions[0].status, "PROVISIONING");
    assert_eq!(sessions[1].id, "old-session");
    let mut output = Vec::new();
    display(&mut output, &sessions).unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(
        output.contains("SESSION ID")
            && output.contains("new-session")
            && output.contains("old-session")
    );
    assert!(output.contains("2026-10-13T18:44:38+02:00"));
}

#[test]
fn empty_cluster_is_clear_and_does_not_describe_tasks() {
    let aws = fake(Vec::new());
    let sessions = list(&aws, &config()).unwrap();
    let mut output = Vec::new();
    display(&mut output, &sessions).unwrap();
    assert_eq!(output, b"No active CLI sessions.\n");
    assert_eq!(aws.calls.borrow().len(), 2);
}

#[test]
fn all_listed_tasks_are_described_in_batches_of_at_most_one_hundred() {
    let aws = fake((0..205).map(|i| json!({"taskArn":format!("arn/task/{i}"), "startedBy":format!("session-{i}"), "lastStatus":"RUNNING"})).collect());
    assert_eq!(list(&aws, &config()).unwrap().len(), 205);
    let calls = aws.calls.borrow();
    let sizes: Vec<_> = calls
        .iter()
        .filter(|c| c[1] == "describe-tasks")
        .map(|c| c.len() - 5)
        .collect();
    assert_eq!(sizes, vec![100, 100, 5]);
}

#[test]
fn missing_task_is_tolerated_but_other_description_failures_are_reported() {
    let mut aws = fake(vec![
        json!({"taskArn":"arn/task/one", "lastStatus":"RUNNING"}),
    ]);
    aws.failures = json!([{"reason":"MISSING"}]);
    assert_eq!(list(&aws, &config()).unwrap()[0].id, "-");
    aws.failures = json!([{"reason":"ACCESS_DENIED"}]);
    assert!(list(&aws, &config()).is_err());
}
