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
        .env_remove("ULTRAFINANCE_DATABASE_URL")
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
        json!({"merchant":{"status":"unresolved","data":null},"location":{"status":"unresolved","data":null}})
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
    let history = run(
        &[
            "--database",
            db,
            "logs",
            "--status",
            "matched",
            "--merchant-id",
            merchant["id"].as_str().unwrap(),
        ],
        None,
    );
    assert!(
        history.status.success(),
        "{}",
        String::from_utf8_lossy(&history.stderr)
    );
    let logs: Value = serde_json::from_slice(&history.stdout).unwrap();
    assert_eq!(logs.as_array().unwrap().len(), 1);
    assert_eq!(logs[0]["data"]["method"], "exact");
    assert_eq!(
        logs[0]["data"]["request"]["description"],
        "julius café bromont"
    );
    assert_eq!(logs[0]["data"]["response"], result);

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn dataset_import_apply_and_refresh_preserve_merchant_ids() {
    let root = std::env::temp_dir().join(format!("ultra-import-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let input = root.join("source.csv");
    std::fs::write(&input, "id,name,parent_id,website_url,transaction_text_examples,transaction_text_regexp\nadidas,Adidas,,https://adidas.com,[`ADIDAS`],ADIDAS\n").unwrap();
    let db = root.join("catalog.sqlite");
    let args = [
        "datasets",
        "import",
        "--source",
        "open-enrichment",
        "--input",
        input.to_str().unwrap(),
        "--output",
        root.to_str().unwrap(),
    ];
    let output = run(&args, None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let prepared: Value = serde_json::from_slice(&output.stdout).unwrap();
    let knowledge = std::path::Path::new(prepared["path"].as_str().unwrap()).join("knowledge.json");
    assert!(run(&args, None).status.success());
    let apply = [
        "--database",
        db.to_str().unwrap(),
        "datasets",
        "apply",
        knowledge.to_str().unwrap(),
    ];
    assert!(run(&apply, None).status.success());
    let list = [
        "--database",
        db.to_str().unwrap(),
        "merchants",
        "list",
        "--json",
    ];
    let before = run(&list, None);
    assert!(before.status.success());
    assert!(run(&apply, None).status.success());
    let after = run(&list, None);
    assert_eq!(
        serde_json::from_slice::<Value>(&before.stdout).unwrap(),
        serde_json::from_slice::<Value>(&after.stdout).unwrap()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_eval_uses_latest_holdouts_and_summarizes_unlabeled_cases() {
    let root = std::env::temp_dir().join(format!("ultra-eval-all-{}", uuid::Uuid::new_v4()));
    let suites = root.join("suites");
    let datasets = root.join("datasets");
    std::fs::create_dir_all(&suites).unwrap();
    std::fs::write(suites.join("smoke.json"), r#"{"version":1,"name":"smoke","cases":[{"id":"known-unresolved","request":{"description":"ZZQQXX"},"expected":{"status":"unresolved"}}]}"#).unwrap();
    for (version, seconds) in [("old", 100), ("new", 200)] {
        let path = datasets.join("demo").join(version);
        std::fs::create_dir_all(&path).unwrap();
        let manifest = path.join("manifest.json");
        std::fs::write(&manifest, r#"{"holdout_samples":2,"region":"global"}"#).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&manifest)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds)),
            )
            .unwrap();
        let samples = if version == "old" {
            "invalid old snapshot".to_string()
        } else {
            ["ZZQQXX", "WWVVZZ"].iter().enumerate().map(|(id, description)| serde_json::to_string(&json!({"id":id.to_string(),"request":{"description":description},"expected":null,"category":null,"source":"demo","label_origin":"unlabeled"})).unwrap()).collect::<Vec<_>>().join("\n")
        };
        std::fs::write(path.join("holdout.jsonl"), samples).unwrap();
        std::fs::write(path.join("development.jsonl"), "not an eval input").unwrap();
    }
    let reports = root.join("reports");
    let database = root.join("catalog.sqlite");
    let args = [
        "--database",
        database.to_str().unwrap(),
        "eval",
        "--all",
        "--mode",
        "enrich",
        "--datasets-dir",
        datasets.to_str().unwrap(),
        "--suites-dir",
        suites.to_str().unwrap(),
        "--output",
        reports.to_str().unwrap(),
    ];
    let output = run(&args, None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let table = String::from_utf8(output.stdout).unwrap();
    for label in [
        "suite/smoke",
        "dataset/demo",
        "Total",
        "Match rate",
        "Accuracy",
    ] {
        assert!(table.contains(label), "{table}");
    }
    let folder = std::fs::read_dir(&reports)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let summary: Value =
        serde_json::from_str(&std::fs::read_to_string(folder.join("summary.json")).unwrap())
            .unwrap();
    assert_eq!(summary["suites"].as_array().unwrap().len(), 2);
    assert_eq!(summary["metrics"]["cases"], 3);
    assert_eq!(summary["metrics"]["unlabeled"], 2);
    assert_eq!(summary["metrics"]["accuracy"], 1.0);
    assert_eq!(summary["metrics"]["unresolved"], 3);
    assert!(
        summary["suites"][1]["input"]
            .as_str()
            .unwrap()
            .contains("new/holdout.jsonl")
    );
    assert!(!run(&["eval", "--all", "--samples"], None).status.success());
    assert!(
        !run(&["eval", "evals/smoke.json", "--all"], None)
            .status
            .success()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn enrich_batch_validates_envelopes_and_outputs_partial_results_before_failing() {
    let directory =
        std::env::temp_dir().join(format!("ultrafinance-bulk-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let database = directory.join("catalog.sqlite");
    let db = database.to_str().unwrap();
    assert!(
        run(
            &[
                "--database",
                db,
                "merchants",
                "add",
                "--id",
                "alpha",
                "--name",
                "Alpha Cafe"
            ],
            None
        )
        .status
        .success()
    );
    let output = run(
        &["--database", db, "enrich-batch", "--input", "-"],
        Some(
            r#"{"transactions":[{"description":"Alpha Cafe"},{"description":""},{"description":"Alpha Cafe PURCHASE"}]}"#,
        ),
    );
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["results"][0]["data"]["merchant"]["data"]["id"],
        "alpha"
    );
    assert_eq!(value["results"][1]["code"], "invalid_request");
    assert_eq!(value["results"][2]["code"], "enrichment_failed");
    let output = run(
        &["enrich-batch", "--input", "-", "--dry-run"],
        Some(r#"{"transactions":[{"description":"test","extra":{"nested":{"value":1}}}]}"#),
    );
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["transactions"][0]["extra"]["nested"]
            ["value"],
        1
    );
    for input in [
        r#"{"transactions":[]}"#,
        r#"{"transactions":[{"description":"x"}],"unknown":true}"#,
    ] {
        let output = run(&["enrich-batch", "--input", "-", "--dry-run"], Some(input));
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn locations_import_list_eval_and_structured_flags_work() {
    let directory =
        std::env::temp_dir().join(format!("ultrafinance-locations-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let database = directory.join("catalog.sqlite");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let merchants = root.join("data/locations/merchants.example.json");
    let outlets = root.join("data/locations/open-enrichment-au.json");
    let suite = root.join("evals/location-smoke.json");
    for args in [
        vec![
            "--database",
            database.to_str().unwrap(),
            "merchants",
            "import",
            merchants.to_str().unwrap(),
            "--source",
            "open-enrichment",
        ],
        vec![
            "--database",
            database.to_str().unwrap(),
            "locations",
            "import",
            outlets.to_str().unwrap(),
        ],
        vec![
            "--database",
            database.to_str().unwrap(),
            "locations",
            "import",
            outlets.to_str().unwrap(),
        ],
    ] {
        let output = run(&args, None);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = run(
        &[
            "--database",
            database.to_str().unwrap(),
            "merchants",
            "list",
            "--json",
        ],
        None,
    );
    let merchants: Value = serde_json::from_slice(&output.stdout).unwrap();
    let merchant_id = merchants["merchants"][0]["id"].as_str().unwrap();
    let output = run(
        &[
            "--database",
            database.to_str().unwrap(),
            "locations",
            "list",
            merchant_id,
        ],
        None,
    );
    let records: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(records.as_array().unwrap().len(), 2);
    assert!(
        records[0]["location"]["id"]
            .as_str()
            .unwrap()
            .starts_with("loc_")
    );
    let output = run(
        &[
            "--database",
            database.to_str().unwrap(),
            "locations",
            "eval",
            suite.to_str().unwrap(),
        ],
        None,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["outlet_accuracy"], 1.0);
    assert_eq!(report["geographic_field_accuracy"], 1.0);
    assert_eq!(report["unresolved_accuracy"], 1.0);
    let output = run(
        &[
            "enrich",
            "UNKNOWN",
            "--location",
            r#"{"city":"Toronto","country":"CA"}"#,
            "--dry-run",
        ],
        None,
    );
    assert!(output.status.success());
    let request: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(request["location"]["city"], "Toronto");
    std::fs::remove_dir_all(directory).unwrap();
}
