use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn run(args: &[&str], stdin: Option<&str>) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ultrafinance"));
    command
        .args(args)
        .env_remove("TYPESAFE_API_KEY")
        .env_remove("ULTRAFINANCE_DB")
        .env_remove("ULTRAFINANCE_MERCHANTS")
        .env_remove("JEV_MODEL")
        .env_remove("ULTRAFINANCE_MATCH_THRESHOLD")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    } else {
        drop(child.stdin.take());
    }
    child.wait_with_output().unwrap()
}

#[test]
fn flags_and_stdin_preserve_nested_evidence() {
    let output = run(
        &[
            "enrich",
            "LS",
            "--country",
            "CA",
            "--amount",
            "-142.97",
            "--extra",
            r#"{"bank":{"category":["Restaurants"]}}"#,
            "--dry-run",
        ],
        None,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["extra"]["bank"]["category"][0], "Restaurants");
    assert_eq!(value["amount"], "-142.97");
    let output = run(
        &["enrich", "--input", "-", "--dry-run"],
        Some(r#"{"description":"LS","extra":{"counterparties":[{"name":"Cafe"}]}}"#),
    );
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["extra"]["counterparties"][0]["name"], "Cafe");
}

#[test]
fn invalid_requests_and_conflicting_modes_exit_nonzero() {
    for args in [
        vec!["enrich", " ", "--dry-run"],
        vec!["enrich", "LS", "--extra", "[]", "--dry-run"],
        vec!["enrich", "--input", "-", "--country", "CA"],
    ] {
        let output = run(&args, Some(r#"{"description":"LS"}"#));
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    let output = run(
        &["enrich", "--input", "-", "--dry-run"],
        Some(r#"{"description":"LS","contry":"CA"}"#),
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid request JSON"));
}

#[test]
fn country_exclusion_returns_unresolved_without_provider_credentials() {
    // The example fixture is Canadian, so US input must never trigger a Jev call.
    let catalog = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../data/merchants.example.json"
    );
    let output = run(
        &["enrich", "LS", "--country", "US", "--merchants", catalog],
        None,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"merchant":{"status":"unresolved","data":null}})
    );
}

#[test]
fn database_persists_aliases_and_exact_matches_without_a_key() {
    let directory =
        std::env::temp_dir().join(format!("ultrafinance-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let database = directory.join("merchants.sqlite");
    let db = database.to_str().unwrap();
    let added = run(
        &[
            "--database",
            db,
            "merchants",
            "add",
            "--name",
            "Julius Café",
            "--country",
            "CA",
            "--alias",
            "JULIUS CAFE BROMONT",
        ],
        None,
    );
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let merchant: Value = serde_json::from_slice(&added.stdout).unwrap();
    let searched = run(
        &[
            "--database",
            db,
            "merchants",
            "search",
            "Julus cafe",
            "--country",
            "CA",
        ],
        None,
    );
    assert!(searched.status.success());
    let candidates: Value = serde_json::from_slice(&searched.stdout).unwrap();
    assert_eq!(candidates[0]["merchant"]["id"], merchant["id"]);
    assert_eq!(candidates[0]["exact"], false);
    let enriched = run(
        &[
            "--database",
            db,
            "enrich",
            "julius café bromont",
            "--country",
            "CA",
        ],
        None,
    );
    assert!(
        enriched.status.success(),
        "{}",
        String::from_utf8_lossy(&enriched.stderr)
    );
    let result: Value = serde_json::from_slice(&enriched.stdout).unwrap();
    assert_eq!(result["merchant"]["status"], "matched");
    assert_eq!(result["merchant"]["data"]["id"], merchant["id"]);
    std::fs::remove_dir_all(directory).unwrap();
}
