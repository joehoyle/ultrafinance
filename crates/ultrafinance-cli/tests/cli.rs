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
        .env(
            "ULTRAFINANCE_DATABASE_URL",
            std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
                .unwrap_or_else(|_| ultrafinance_core::store::LOCAL_DATABASE_URL.into()),
        )
        .env_remove("ULTRAFINANCE_MERCHANTS")
        .env_remove("JEV_MODEL")
        .env_remove("ULTRAFINANCE_MATCH_THRESHOLD")
        .env_remove("ULTRAFINANCE_DISCOVERY_URL")
        .env_remove("ULTRAFINANCE_DISCOVERY_API_KEY")
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
fn missing_subcommands_show_contextual_help() {
    for group in [
        None,
        Some("infra"),
        Some("database"),
        Some("datasets"),
        Some("locations"),
        Some("merchants"),
    ] {
        for global_flags in [false, true] {
            let mut args = Vec::new();
            if global_flags {
                args.extend(["--database-url", "postgresql://unused"]);
            }
            if let Some(group) = group {
                args.push(group);
            }
            let output = run(&args, None);
            let help = String::from_utf8(output.stdout).unwrap();
            assert!(
                output.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stderr.is_empty());
            let usage = match group {
                Some(group) => format!("Usage: ultrafinance {group}"),
                None => "Usage: ultrafinance".to_owned(),
            };
            assert!(help.contains(&usage), "{help}");
            assert!(help.contains("Commands:"), "{help}");
            assert!(help.contains("--help"), "{help}");
        }
    }
    let output = Command::new(env!("CARGO_BIN_EXE_ultrafinance"))
        .env("ULTRAFINANCE_DATABASE_URL", "postgres://unused")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Commands:")
    );
}

#[cfg(unix)]
#[test]
fn infra_routes_logs_and_stops_deploy_before_build_on_config_failure() {
    use std::os::unix::fs::PermissionsExt;
    let directory =
        std::env::temp_dir().join(format!("ultrafinance-infra-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(directory.join("deploy")).unwrap();
    std::fs::create_dir_all(directory.join("infra")).unwrap();
    std::fs::create_dir_all(directory.join("bin")).unwrap();
    std::fs::write(directory.join("infra/outputs.tf"), "").unwrap();
    std::fs::write(directory.join("Cargo.toml"), "").unwrap();
    for (file, body) in [
        ("deploy/deploy.sh", "#!/bin/sh\necho SHOULD_NOT_DEPLOY\n"),
        (
            "bin/tofu",
            "#!/bin/sh\ncase \"$4\" in\naws_profile) echo test-profile;;\naws_region) echo test-region;;\nfunction_name) echo test-function;;\n*) exit 2;;\nesac\n",
        ),
        ("bin/aws", "#!/bin/sh\nprintf '%s\\n' \"$@\"\n"),
    ] {
        let path = directory.join(file);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let invoke = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ultrafinance"))
            .current_dir(&directory)
            .env("PATH", directory.join("bin"))
            // Exercise OpenTofu fixture outputs independently of CI deployment settings.
            .env_remove("AWS_PROFILE")
            .env_remove("AWS_REGION")
            .env_remove("LAMBDA_FUNCTION_NAME")
            .env_remove("ECR_REPOSITORY")
            .env_remove("ULTRAFINANCE_SITE_URL")
            .env(
                "ULTRAFINANCE_DATABASE_URL",
                std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
                    .unwrap_or_else(|_| ultrafinance_core::store::LOCAL_DATABASE_URL.into()),
            )
            .args(args)
            .output()
            .unwrap()
    };
    let logs = invoke(&["infra", "logs", "--since", "2h", "--follow"]);
    assert!(
        logs.status.success(),
        "{}",
        String::from_utf8_lossy(&logs.stderr)
    );
    assert_eq!(
        String::from_utf8(logs.stdout).unwrap(),
        "--profile\ntest-profile\n--region\ntest-region\nlogs\ntail\n/aws/lambda/test-function\n--since\n2h\n--format\nshort\n--follow\n"
    );
    let deploy = invoke(&["infra", "deploy"]);
    assert!(!deploy.status.success());
    assert!(deploy.stdout.is_empty());
    assert!(String::from_utf8_lossy(&deploy.stderr).contains("repository_url"));
    let image = format!(
        "123456789012.dkr.ecr.ca-central-1.amazonaws.com/ultrafinance@sha256:{}",
        "a".repeat(64)
    );
    // CI has OIDC credentials and no local OpenTofu state or Docker build.
    std::fs::write(
        directory.join("bin/aws"),
        "#!/bin/sh\nif [ \"$1\" != --region ] || [ \"$2\" != ci-region ]; then exit 2; fi\necho 'An error occurred (CIReleaseReached) when calling GetAlias' >&2\nexit 1\n",
    )
    .unwrap();
    let ci = Command::new(env!("CARGO_BIN_EXE_ultrafinance"))
        .current_dir(&directory)
        .env("PATH", directory.join("bin"))
        .env("AWS_PROFILE", "")
        .env("AWS_REGION", "ci-region")
        .env("LAMBDA_FUNCTION_NAME", "ci-function")
        .args(["infra", "deploy", "--image", &image])
        .output()
        .unwrap();
    assert!(!ci.status.success());
    assert!(String::from_utf8_lossy(&ci.stderr).contains("CIReleaseReached"));
    let shell = invoke(&["infra", "cli"]);
    assert!(!shell.status.success());
    assert!(String::from_utf8_lossy(&shell.stderr).contains("interactive terminal"));
    std::fs::remove_dir_all(directory).unwrap();
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
fn short_descriptor_remains_unresolved_without_provider_credentials() {
    // A short descriptor supplies no matching evidence, regardless of market.
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
    let database = ultrafinance_core::store::MerchantStore::temporary().unwrap();
    let db = database.temporary_url();
    let added = run(
        &[
            "--database-url",
            db,
            "merchants",
            "add",
            "--name",
            "Julius Café",
            "--market",
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
            "--database-url",
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
            "--database-url",
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
            "--database-url",
            db,
            "logs",
            "--status",
            "matched",
            "--merchant-id",
            merchant["id"].as_str().unwrap(),
            "--json",
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

    let history_table = run(&["--database-url", db, "logs", "--status", "matched"], None);
    assert!(history_table.status.success());
    let table = String::from_utf8(history_table.stdout).unwrap();
    for expected in [
        "Created (UTC)",
        "Status",
        "Description",
        "Merchant",
        "Method",
        "Error",
        "matched",
        "julius café bromont",
        "exact",
        "Showing 1–1",
    ] {
        assert!(table.contains(expected), "{table}");
    }
    let empty = run(&["--database-url", db, "logs", "--offset", "1"], None);
    assert!(empty.status.success());
    assert!(
        String::from_utf8(empty.stdout)
            .unwrap()
            .contains("No enrichment logs on this page (offset 1).")
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn dataset_import_apply_and_refresh_preserve_merchant_ids() {
    let root = std::env::temp_dir().join(format!("ultra-import-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let input = root.join("source.csv");
    std::fs::write(&input, "id,name,parent_id,website_url,transaction_text_examples,transaction_text_regexp\nadidas,Adidas,,https://adidas.com,[`ADIDAS`],ADIDAS\n").unwrap();
    let db = ultrafinance_core::store::MerchantStore::temporary().unwrap();
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
        "--database-url",
        db.temporary_url(),
        "datasets",
        "apply",
        knowledge.to_str().unwrap(),
    ];
    assert!(run(&apply, None).status.success());
    let list = [
        "--database-url",
        db.temporary_url(),
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
    let database = ultrafinance_core::store::MerchantStore::temporary().unwrap();
    let args = [
        "--database-url",
        database.temporary_url(),
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
    let database = ultrafinance_core::store::MerchantStore::temporary().unwrap();
    let db = database.temporary_url();
    assert!(
        run(
            &[
                "--database-url",
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
        &["--database-url", db, "enrich-batch", "--input", "-"],
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
    let database = ultrafinance_core::store::MerchantStore::temporary().unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let merchants = root.join("data/locations/merchants.example.json");
    let outlets = root.join("data/locations/open-enrichment-au.json");
    let suite = root.join("evals/location-smoke.json");
    for args in [
        vec![
            "--database-url",
            database.temporary_url(),
            "merchants",
            "import",
            merchants.to_str().unwrap(),
            "--source",
            "open-enrichment",
        ],
        vec![
            "--database-url",
            database.temporary_url(),
            "locations",
            "import",
            outlets.to_str().unwrap(),
        ],
        vec![
            "--database-url",
            database.temporary_url(),
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
            "--database-url",
            database.temporary_url(),
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
            "--database-url",
            database.temporary_url(),
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
            "--database-url",
            database.temporary_url(),
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

#[test]
fn merchant_stats_counts_linked_sources_and_missing_market_evidence() {
    use ultrafinance_core::{import, store::MerchantStore};
    let path = ultrafinance_core::store::MerchantStore::temporary().unwrap();
    let db = path.temporary_url();
    let invoke = |json: bool| {
        let mut args = vec!["--database-url", db, "merchants", "stats"];
        if json {
            args.push("--json");
        }
        let output = run(&args, None);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };
    let empty: Value = serde_json::from_slice(&invoke(true)).unwrap();
    assert_eq!(
        empty,
        json!({"total":0,"manual":0,"without_source":0,"by_source":[],"without_market_evidence":0,"by_market":[],"by_source_region":[]})
    );
    let store = MerchantStore::postgres(db).unwrap();
    let catalog = r#"[{"id":"one","name":"One","markets":["CA"]},{"id":"two","name":"Two","markets":["CA"]}]"#;
    store
        .import(&import::catalog(catalog, "alpha").unwrap())
        .unwrap();
    let id = store.resolve_source("alpha", "one").unwrap().unwrap();
    store.link("alpha", "two", &id).unwrap();
    store
        .import(
            &import::catalog(
                r#"[{"id":"three","name":"Three","markets":["CA"]}]"#,
                "beta",
            )
            .unwrap(),
        )
        .unwrap();
    store.link("beta", "three", &id).unwrap();
    store
        .put(&serde_json::from_value(json!({"id":id,"name":"Corrected","markets":["CA"]})).unwrap())
        .unwrap();
    store
        .put(&serde_json::from_value(json!({"id":"manual-only","name":"Unknown"})).unwrap())
        .unwrap();
    let stats: Value = serde_json::from_slice(&invoke(true)).unwrap();
    assert_eq!(
        stats,
        json!({
            "total":2,"manual":2,"without_source":1,
            "by_source":[{"source":"alpha","merchants":1,"records":2},{"source":"beta","merchants":1,"records":1}],
            "without_market_evidence":1,"by_market":[{"market":"CA","merchants":1}],"by_source_region":[]
        })
    );
    let text = String::from_utf8(invoke(false)).unwrap();
    for expected in [
        "Total merchants: 2",
        "Manual entries / corrections: 2",
        "Without imported source: 1",
        "alpha",
        "beta",
        "No market evidence: 1",
        "CA",
    ] {
        assert!(text.contains(expected), "{text}");
    }
}

#[test]
fn merchant_markets_flags_replace_country_and_exact_matches_remain_eligible() {
    let path = ultrafinance_core::store::MerchantStore::temporary().unwrap();
    let db = path.temporary_url();
    let added = run(
        &[
            "--database-url",
            db,
            "merchants",
            "add",
            "--name",
            "Example Brand",
            "--market",
            "US",
            "--market",
            "CA",
            "--market",
            "CA",
        ],
        None,
    );
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let merchant: Value = serde_json::from_slice(&added.stdout).unwrap();
    assert_eq!(merchant["markets"], json!(["CA", "US"]));
    assert!(merchant.get("country").is_none());
    let listed = run(
        &[
            "--database-url",
            db,
            "merchants",
            "list",
            "--market",
            "CA",
            "--json",
        ],
        None,
    );
    assert!(listed.status.success());
    let page: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(page["total"], 1);
    assert_eq!(
        page["merchants"][0]["market_evidence"][0]["source"],
        "manual"
    );
    let enriched = run(
        &[
            "--database-url",
            db,
            "enrich",
            "Example Brand",
            "--country",
            "DE",
        ],
        None,
    );
    assert!(
        enriched.status.success(),
        "{}",
        String::from_utf8_lossy(&enriched.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&enriched.stdout).unwrap()["merchant"]["status"],
        "matched"
    );
    for args in [
        vec!["merchants", "add", "--name", "Brand", "--country", "CA"],
        vec!["merchants", "list", "--country", "CA"],
    ] {
        assert!(!run(&args, None).status.success());
    }
}

#[test]
fn infra_cli_latest_is_documented_and_conflicts_with_explicit_image() {
    let help = run(&["infra", "cli", "--help"], None);
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    assert!(text.contains("--latest"));
    assert!(text.contains("newest published Lambda version"));
    let conflict = run(&["infra", "cli", "--latest", "--image", "example"], None);
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("cannot be used with"));
}

#[test]
fn dedupe_cli_empty_catalog_and_provider_failure_are_safe() {
    let dir =
        std::env::temp_dir().join(format!("ultrafinance-dedupe-cli-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let database = ultrafinance_core::store::MerchantStore::temporary().unwrap();
    let db = database.temporary_url();
    let output = run(&["--database-url", db, "merchants", "dedupe"], None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["candidates"], 0);
    assert_eq!(report["dry_run"], false);
    let file = dir.join("merchants.json");
    std::fs::write(&file,r#"[{"id":"a","name":"Brand","website":"https://example.com"},{"id":"b","name":"Brand","website":"https://www.example.com"}]"#).unwrap();
    assert!(
        run(
            &[
                "--database-url",
                db,
                "merchants",
                "import",
                file.to_str().unwrap()
            ],
            None
        )
        .status
        .success()
    );
    for extra in [
        vec![],
        vec!["--dry-run"],
        vec!["--threshold", "NaN"],
        vec!["--threshold", "0.2"],
    ] {
        let mut args = vec!["--database-url", db, "merchants", "dedupe"];
        args.extend(extra);
        assert!(!run(&args, None).status.success());
        let output = run(
            &["--database-url", db, "merchants", "stats", "--json"],
            None,
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap()["total"],
            2
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn version_flags_print_embedded_build_identity_without_opening_a_database() {
    for flag in ["--version", "-V"] {
        let output = run(&["--database-url", "invalid", flag], None);
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("ultrafinance {}\n", env!("ULTRAFINANCE_CLI_VERSION"))
        );
    }
}

#[test]
fn interpretation_and_reviewed_resolution_commands_preserve_context_and_allow_revocation() {
    fn json(args: &[&str]) -> Value {
        let output = run(args, None);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    let parsed = json(&["interpret", "SQ *JULIUS CAFE BROMONT 00482"]);
    assert_eq!(parsed["hypotheses"][1]["merchant_text"], "JULIUS CAFE");
    let path = ultrafinance_core::store::MerchantStore::temporary().unwrap();
    let db = path.temporary_url();
    let store = ultrafinance_core::store::MerchantStore::postgres(db).unwrap();
    let merchant =
        serde_json::from_value(json!({"id":"cafe","name":"Royal Cafe","markets":["CA"]})).unwrap();
    store.put(&merchant).unwrap();
    let request =
        serde_json::from_value(json!({"description":"opaque zxmq","country":"CA"})).unwrap();
    let mapping = ultrafinance_core::resolution::Resolution::supported(&request, merchant, vec![]);
    store.save_resolution(&mapping).unwrap();
    drop(store);
    let listed = json(&["--database-url", db, "resolutions", "list"]);
    assert_eq!(listed[0]["verified"], false);
    assert!(
        !run(
            &[
                "--database-url",
                db,
                "resolutions",
                "confirm",
                &mapping.id,
                "--evidence",
                " "
            ],
            None
        )
        .status
        .success()
    );
    assert!(
        run(
            &[
                "--database-url",
                db,
                "resolutions",
                "confirm",
                &mapping.id,
                "--evidence",
                "Official receipt verified"
            ],
            None
        )
        .status
        .success()
    );
    let response = json(&[
        "--database-url",
        db,
        "enrich",
        "opaque zxmq",
        "--country",
        "CA",
    ]);
    assert_eq!(response["merchant"]["data"]["id"], "cafe");
    let other = json(&[
        "--database-url",
        db,
        "enrich",
        "opaque zxmq",
        "--country",
        "US",
    ]);
    assert_eq!(other["merchant"]["status"], "unresolved");
    assert!(
        run(
            &["--database-url", db, "resolutions", "revoke", &mapping.id],
            None
        )
        .status
        .success()
    );
    assert!(
        json(&["--database-url", db, "resolutions", "list"])
            .as_array()
            .unwrap()
            .is_empty()
    );
}
