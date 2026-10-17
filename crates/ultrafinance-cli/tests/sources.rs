use serde_json::Value;
use std::process::{Command, Output};
fn run(root: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ultrafinance"))
        .args(["sources", "--cache-dir"])
        .arg(root.join("cache"))
        .args(args)
        .env("ULTRAFINANCE_DATABASE_URL", "postgresql://unused")
        .env_remove("ULTRAFINANCE_FOURSQUARE_TOKEN")
        .output()
        .unwrap()
}
fn json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn foursquare_prepares_place_identities_without_manual_review_or_transaction_labels() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("places.csv");
    let mut csv = csv::Writer::from_writer(vec![]);
    csv.write_record([
        "fsq_place_id",
        "name",
        "country",
        "date_closed",
        "fsq_category_ids",
    ])
    .unwrap();
    for (id, name) in [
        ("a", "Starbucks Bromont"),
        ("b", "Starbucks Toronto"),
        ("c", "Central Cafe"),
        ("d", "Central Cafe"),
    ] {
        csv.write_record([id, name, "CA", "", "[\"restaurant\"]"])
            .unwrap();
    }
    std::fs::write(&input, csv.into_inner().unwrap()).unwrap();
    let output = root.path().join("bundles");
    let report = json(run(
        root.path(),
        &[
            "import",
            "foursquare",
            "--input",
            input.to_str().unwrap(),
            "--region",
            "ca",
            "--output",
            output.to_str().unwrap(),
            "--dry-run",
            "--merchants-only",
        ],
    ));
    assert_eq!(report["merchants_only"], true);
    assert_eq!(report["manifest"]["merchant_records"], 4);
    assert_eq!(report["manifest"]["retained_places"], 4);
    assert_eq!(report["manifest"]["holdout_samples"], 0);
    assert!(report["bundle"].is_null());
    let checkpoint = std::path::Path::new(report["checkpoint"].as_str().unwrap());
    assert!(checkpoint.join("NOTICE.txt").is_file());
    assert!(checkpoint.join("LICENSE.txt").is_file());
    assert!(!checkpoint.join("knowledge.json").exists());
    let offline = json(run(
        root.path(),
        &[
            "import",
            "foursquare",
            "--region",
            "ca",
            "--offline",
            "--merchants-only",
            "--output",
            output.to_str().unwrap(),
            "--dry-run",
        ],
    ));
    assert_eq!(offline["checkpoint"], report["checkpoint"]);
    let rejected = run(
        root.path(),
        &[
            "import",
            "foursquare",
            "--input",
            input.to_str().unwrap(),
            "--examples",
            "nonexistent-brands.json",
            "--dry-run",
        ],
    );
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("do not accept --examples"));
    let download = run(root.path(), &["download", "foursquare"]);
    assert!(!download.status.success());
    assert!(String::from_utf8_lossy(&download.stderr).contains("authenticated Places Portal"));
    let eval = run(root.path(), &["eval", "foursquare", "--offline"]);
    assert!(!eval.status.success());
    assert!(String::from_utf8_lossy(&eval.stderr).contains("not transaction evaluation samples"));
}

#[test]
fn foursquare_import_refresh_keeps_place_and_location_identities() {
    let store = ultrafinance_core::store::MerchantStore::temporary().unwrap();
    let input = "fsq_place_id,name,country,address,date_closed,fsq_category_ids\na,Starbucks Bromont,CA,1 Main St,,\"[\"\"restaurant\"\"]\"\nb,Starbucks Toronto,CA,2 Main St,,\"[\"\"restaurant\"\"]\"\n";
    let bundle = ultrafinance_core::datasets::prepare(
        ultrafinance_core::datasets::Source::Foursquare,
        input,
        None,
        "ca",
    )
    .unwrap();
    assert_eq!(store.import_delta(&bundle.records).unwrap().added, 2);
    let id = store
        .resolve_source("foursquare", "place:a")
        .unwrap()
        .unwrap();
    assert_eq!(store.stats().unwrap().total, 2);
    let locations = store.locations(&id).unwrap();
    assert_eq!(locations.len(), 1);
    assert_eq!(store.get(&id).unwrap().unwrap().markets, ["CA"]);
    assert_eq!(store.import_delta(&bundle.records).unwrap().unchanged, 2);
    let refreshed = store.locations(&id).unwrap();
    assert_eq!(
        locations.iter().map(|r| &r.location.id).collect::<Vec<_>>(),
        refreshed.iter().map(|r| &r.location.id).collect::<Vec<_>>()
    );
    let updated = input.replace("Starbucks Bromont", "Starbucks Coffee Bromont");
    let bundle = ultrafinance_core::datasets::prepare(
        ultrafinance_core::datasets::Source::Foursquare,
        &updated,
        None,
        "ca",
    )
    .unwrap();
    assert_eq!(store.import_delta(&bundle.records).unwrap().updated, 1);
    assert_eq!(
        store
            .resolve_source("foursquare", "place:a")
            .unwrap()
            .unwrap(),
        id
    );
    assert_eq!(
        store.get(&id).unwrap().unwrap().name,
        "Starbucks Coffee Bromont"
    );
    // Imported names do not become trusted exact aliases.
    let candidates = store
        .search("Starbucks Coffee Bromont", Some("CA"), 10)
        .unwrap();
    assert!(!candidates[0].trusted);
}
#[test]
fn registry_and_offline_browsing_work_without_a_database() {
    let root = tempfile::tempdir().unwrap();
    let registry = json(run(root.path(), &["list", "--json"]));
    assert_eq!(registry.as_array().unwrap().len(), 7);
    assert_eq!(registry[3]["download"], "automatic");
    let moneyvis = json(run(root.path(), &["show", "moneyvis"]));
    assert_eq!(
        moneyvis["downloads"][0]["url"],
        "https://raw.githubusercontent.com/thevisgroup/MoneyVis/master/data/data.csv"
    );
    assert_eq!(
        json(run(root.path(), &["show", "merchant-studio"]))["snapshot"],
        Value::Null
    );
    let input = root.path().join("input.csv");
    std::fs::write(&input, "description,category\nPAYMENT $12,Other\n").unwrap();
    let output = root.path().join("bundles");
    let first = json(run(
        root.path(),
        &[
            "import",
            "dodatathings",
            "--input",
            input.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--dry-run",
        ],
    ));
    assert_eq!(first["manifest"]["merchant_records"], 0);
    let raw = json(run(root.path(), &["raw", "dodatathings", "--limit", "1"]));
    assert_eq!(raw["rows"][0]["description"], "PAYMENT $12");
    let second = json(run(
        root.path(),
        &[
            "import",
            "dodatathings",
            "--offline",
            "--output",
            output.to_str().unwrap(),
            "--dry-run",
        ],
    ));
    assert_eq!(first["bundle"], second["bundle"]);
    assert!(
        !run(root.path(), &["raw", "dodatathings", "--limit", "0"])
            .status
            .success()
    );
}
#[test]
fn evaluation_catalog_requires_explicit_opt_in_and_missing_cache_has_a_clear_error() {
    let root = tempfile::tempdir().unwrap();
    let guarded = run(root.path(), &["import", "business-transactions"]);
    assert!(!guarded.status.success());
    assert!(String::from_utf8_lossy(&guarded.stderr).contains("--evaluation"));
    let missing = run(
        root.path(),
        &["import", "merchant-studio", "--offline", "--dry-run"],
    );
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no local snapshot"));
}

#[test]
fn source_import_reports_real_deltas_and_records_keep_stable_mappings() {
    let store = ultrafinance_core::store::MerchantStore::temporary().unwrap();
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("merchants.json");
    let examples = root.path().join("examples.json");
    let bundles = root.path().join("bundles");
    std::fs::write(&examples, r#"{"schemaVersion":"1.1.0","descriptors":[]}"#).unwrap();
    let write_input = |name| {
        std::fs::write(
            &input,
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion":"1.1.0", "merchants":[
                    {"id":"a","canonicalName":name,"aliases":["ALPHA BILL"]},
                    {"id":"b","canonicalName":"Beta"}
                ]
            }))
            .unwrap(),
        )
        .unwrap()
    };
    let import = || {
        json(
            Command::new(env!("CARGO_BIN_EXE_ultrafinance"))
                .args(["sources", "--cache-dir"])
                .arg(root.path().join("cache"))
                .args(["import", "merchant-studio", "--input"])
                .arg(&input)
                .arg("--examples")
                .arg(&examples)
                .arg("--output")
                .arg(&bundles)
                .env("ULTRAFINANCE_DATABASE_URL", store.temporary_url())
                .output()
                .unwrap(),
        )
    };
    write_input("Alpha");
    assert_eq!(
        import()["delta"],
        serde_json::json!({"added":2,"updated":0,"unchanged":0})
    );
    let id = store
        .resolve_source("merchant-studio", "a")
        .unwrap()
        .unwrap();
    assert_eq!(
        import()["delta"],
        serde_json::json!({"added":0,"updated":0,"unchanged":2})
    );
    write_input("Alpha Updated");
    assert_eq!(
        import()["delta"],
        serde_json::json!({"added":0,"updated":1,"unchanged":1})
    );
    let rows = json(
        Command::new(env!("CARGO_BIN_EXE_ultrafinance"))
            .args([
                "sources",
                "records",
                "merchant-studio",
                "--external-id",
                "a",
            ])
            .env("ULTRAFINANCE_DATABASE_URL", store.temporary_url())
            .output()
            .unwrap(),
    );
    assert_eq!(rows[0]["merchant_id"], id);
    assert_eq!(rows[0]["record"]["merchant"]["name"], "Alpha Updated");
}

#[test]
fn source_eval_prepares_cached_moneyvis_and_runs_unlabeled_holdout_without_imports() {
    let store = ultrafinance_core::store::MerchantStore::temporary().unwrap();
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("moneyvis.csv");
    let bundles = root.path().join("bundles");
    let report_path = root.path().join("reports/moneyvis.json");
    let mut csv =
        String::from("Transaction Description,Debit Amount,Credit Amount,Transaction Type\n");
    for i in 0..30 {
        csv.push_str(&format!("SAMPLE SHOP {i},12.00,,DEB\n"));
    }
    std::fs::write(&input, csv).unwrap();
    let prepared = json(run(
        root.path(),
        &[
            "import",
            "moneyvis",
            "--input",
            input.to_str().unwrap(),
            "--output",
            bundles.to_str().unwrap(),
            "--dry-run",
        ],
    ));
    assert!(prepared["manifest"]["holdout_samples"].as_u64().unwrap() > 0);
    // Evaluation also works when the prepared bundle is absent; the raw cache is enough.
    std::fs::remove_dir_all(&bundles).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ultrafinance"))
        .args(["sources", "--cache-dir"])
        .arg(root.path().join("cache"))
        .args([
            "eval",
            "moneyvis",
            "--offline",
            "--details",
            "--limit",
            "1",
            "--datasets-dir",
        ])
        .arg(&bundles)
        .arg("--output")
        .arg(&report_path)
        .env("ULTRAFINANCE_DATABASE_URL", store.temporary_url())
        .env_remove("TYPESAFE_API_KEY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("coverage, not accuracy"));
    let report: Value = serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["metrics"]["cases"], 1);
    assert_eq!(report["metrics"]["unlabeled"], 1);
    assert_eq!(report["metrics"]["labeled_merchants"], 0);
    assert_eq!(store.stats().unwrap().total, 0);
    let details = String::from_utf8_lossy(&output.stdout);
    assert!(details.contains("Description:"));
    assert!(details.contains("SAMPLE SHOP"));
    assert!(details.contains("Catalog candidates: none"));
    assert!(details.contains("Match: not evaluated (search mode)"));
    let description = report["results"][0]["request"]["description"]
        .as_str()
        .unwrap();
    store
        .put(&ultrafinance_core::Merchant {
            id: "sample-shop".into(),
            name: description.into(),
            markets: vec![],

            website: None,
            logo_url: None,
            logo_source: None,
            aliases: vec![],
            sources: vec![],
        })
        .unwrap();
    let enriched = Command::new(env!("CARGO_BIN_EXE_ultrafinance"))
        .args(["sources", "--cache-dir"])
        .arg(root.path().join("cache"))
        .args([
            "eval",
            "moneyvis",
            "--offline",
            "--details",
            "--mode",
            "enrich",
            "--limit",
            "1",
            "--datasets-dir",
        ])
        .arg(&bundles)
        .arg("--output")
        .arg(&report_path)
        .env("ULTRAFINANCE_DATABASE_URL", store.temporary_url())
        .env_remove("TYPESAFE_API_KEY")
        .output()
        .unwrap();
    assert!(
        enriched.status.success(),
        "{}",
        String::from_utf8_lossy(&enriched.stderr)
    );
    let details = String::from_utf8_lossy(&enriched.stdout);
    assert!(details.contains("sample-shop"));
    assert!(details.contains("Search similarity"));
    assert!(details.contains("Jev probability"));
    assert!(details.contains("Jev: not called (trusted exact match)"));
    assert!(details.contains("MATCHED"));
    assert!(details.contains(&format!("Matched: {description} [sample-shop]")));
    let report: Value = serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(
        report["results"][0]["candidates"][0]["merchant"]["id"],
        "sample-shop"
    );
    assert_eq!(report["results"][0]["predicted_id"], "sample-shop");
    let missing = run(root.path(), &["eval", "merchant-studio", "--offline"]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no local snapshot"));
    assert!(
        !run(root.path(), &["eval", "moneyvis", "--offline", "--refresh"])
            .status
            .success()
    );
}

#[cfg(unix)]
#[test]
fn lunchmoney_download_invokes_cli_and_preserves_raw_snapshot() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("lunchmoney");
    // Assert pagination, bank metadata, and original split/group rows are requested.
    std::fs::write(&executable, r#"#!/bin/sh
case "$*" in
  "--output json transactions list --start-date "*" --end-date "*" --all --limit 1000 --include-metadata --include-split-parents --include-group-children --exclude-pending") ;;
  *) exit 2 ;;
esac
printf '%s' '{"transactions":[{"id":1,"plaid_account_id":2,"original_name":"BANK ORIGINAL","payee":"Edited"}],"has_more":false}'
"#).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let downloaded = json(run(
        root.path(),
        &[
            "--lunchmoney-cli",
            executable.to_str().unwrap(),
            "download",
            "lunchmoney",
        ],
    ));
    let snapshot = std::path::Path::new(downloaded["snapshot"].as_str().unwrap());
    assert!(snapshot.join("input.json").exists());
    let raw = json(run(root.path(), &["raw", "lunchmoney"]));
    assert_eq!(raw["rows"][0]["original_name"], "BANK ORIGINAL");
    assert_eq!(raw["rows"][0]["payee"], "Edited");
    let prepared = json(run(
        root.path(),
        &[
            "import",
            "lunchmoney",
            "--offline",
            "--dry-run",
            "--output",
            root.path().join("datasets").to_str().unwrap(),
        ],
    ));
    assert_eq!(prepared["manifest"]["merchant_records"], 0);
    std::fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s' '{\"transactions\":[],\"has_more\":true}'\n",
    )
    .unwrap();
    assert!(
        !run(
            root.path(),
            &[
                "--lunchmoney-cli",
                executable.to_str().unwrap(),
                "download",
                "lunchmoney"
            ]
        )
        .status
        .success()
    );
    assert_eq!(
        json(run(root.path(), &["show", "lunchmoney"]))["snapshot"],
        downloaded["snapshot"]
    );
}

#[cfg(unix)]
#[test]
fn foursquare_authenticated_download_preserves_subset_and_hides_credentials() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let engine = root.path().join("duckdb");
    std::fs::write(&engine, r#"#!/usr/bin/env python3
import sys, os, re
assert 'secret-token' not in ' '.join(sys.argv)
assert 'ULTRAFINANCE_FOURSQUARE_TOKEN' not in os.environ
sql = sys.stdin.read()
assert "TOKEN 'secret-token'" in sql
assert "upper(country) = 'CA'" in sql
assert ('LIMIT 3' in sql) if os.environ.get('EXPECT_LIMIT', '3') == '3' else (' LIMIT ' not in sql)
assert ('ORDER BY fsq_place_id' in sql) if os.environ.get('EXPECT_LIMIT', '3') == '3' else ('ORDER BY' not in sql)
assert "SET memory_limit = '512MiB'" in sql
assert "SET temp_directory = '" in sql and 'duckdb-spill' in sql
assert 'SET preserve_insertion_order = false' in sql
assert 'to_json(fsq_category_ids)' in sql
total = re.search(r"TO '([^']+)' \(FORMAT CSV, HEADER FALSE", sql).group(1)
open(total, 'w').write('1\n')
path = re.search(r"TO '([^']+)' \(FORMAT CSV, HEADER TRUE", sql).group(1)
open(path, 'w').write('fsq_place_id,name,country,fsq_category_ids,date_closed\np1,Cafe,CA,"[""4bf58dd8d48988d16d941735""]",\n')
"#).unwrap();
    std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o700)).unwrap();
    let execute = || {
        Command::new(env!("CARGO_BIN_EXE_ultrafinance"))
            .args(["sources", "--cache-dir"])
            .arg(root.path().join("cache"))
            .arg("--duckdb-cli")
            .arg(&engine)
            .args([
                "--foursquare-limit",
                "3",
                "download",
                "foursquare",
                "--region",
                "ca",
            ])
            .env("ULTRAFINANCE_FOURSQUARE_TOKEN", "secret-token")
            .env("ULTRAFINANCE_DUCKDB_MEMORY_LIMIT", "512MiB")
            .output()
            .unwrap()
    };
    let result = execute();
    assert!(!String::from_utf8_lossy(&result.stderr).contains("secret-token"));
    let progress = String::from_utf8_lossy(&result.stderr);
    assert!(progress.contains("Preparing export"));
    assert!(progress.contains("1/1 rows (100.0%)"));
    assert!(progress.contains("MiB") && progress.contains("Export complete"));
    for stage in [
        "Saving CSV",
        "Validating CSV",
        "Hashing CSV",
        "snapshot saved",
    ] {
        assert!(progress.contains(stage), "{progress}");
    }
    assert!(!progress.contains('\u{1b}'));
    let result = json(result);
    let snapshot = std::path::Path::new(result["snapshot"].as_str().unwrap());
    let metadata = std::fs::read_to_string(snapshot.join("download.json")).unwrap();
    assert!(metadata.contains("limit=3"));
    assert!(!metadata.contains("secret-token"));
    assert!(snapshot.join("input.csv").exists());
    let unlimited = json(
        Command::new(env!("CARGO_BIN_EXE_ultrafinance"))
            .args(["sources", "--cache-dir"])
            .arg(root.path().join("unlimited-cache"))
            .arg("--duckdb-cli")
            .arg(&engine)
            .args(["download", "foursquare", "--region", "ca"])
            .env("ULTRAFINANCE_FOURSQUARE_TOKEN", "secret-token")
            .env("EXPECT_LIMIT", "all")
            .env("ULTRAFINANCE_DUCKDB_MEMORY_LIMIT", "512MiB")
            .output()
            .unwrap(),
    );
    let unlimited_metadata = std::fs::read_to_string(
        std::path::Path::new(unlimited["snapshot"].as_str().unwrap()).join("download.json"),
    )
    .unwrap();
    assert!(unlimited_metadata.contains("limit=all"));
    let prepared = json(
        Command::new(env!("CARGO_BIN_EXE_ultrafinance"))
            .args(["sources", "--cache-dir"])
            .arg(root.path().join("cache"))
            .arg("--duckdb-cli")
            .arg(&engine)
            .args([
                "--foursquare-limit",
                "3",
                "import",
                "foursquare",
                "--region",
                "ca",
                "--dry-run",
            ])
            .arg("--output")
            .arg(root.path().join("bundles"))
            .env("ULTRAFINANCE_FOURSQUARE_TOKEN", "secret-token")
            .env("ULTRAFINANCE_DUCKDB_MEMORY_LIMIT", "512MiB")
            .output()
            .unwrap(),
    );
    assert!(prepared["bundle"].is_null());
    assert_eq!(prepared["manifest"]["retained_places"], 1);
    let checkpoint = std::path::Path::new(prepared["checkpoint"].as_str().unwrap());
    assert!(!checkpoint.join("knowledge.json").exists());
    let offline = json(run(
        root.path(),
        &[
            "import",
            "foursquare",
            "--region",
            "ca",
            "--offline",
            "--dry-run",
            "--output",
            root.path().join("offline").to_str().unwrap(),
        ],
    ));
    assert_eq!(prepared["manifest"], offline["manifest"]);
    let latest = std::fs::read(root.path().join("cache/foursquare/ca/latest")).unwrap();
    std::fs::write(&engine, "#!/bin/sh\necho secret-token >&2\nexit 1\n").unwrap();
    let result = execute();
    assert!(!result.status.success());
    assert!(!String::from_utf8_lossy(&result.stderr).contains("secret-token"));
    assert_eq!(
        latest,
        std::fs::read(root.path().join("cache/foursquare/ca/latest")).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn foursquare_download_reports_progress_while_csv_grows() {
    use std::io::{BufRead, BufReader};
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;
    let root = tempfile::tempdir().unwrap();
    let engine = root.path().join("duckdb");
    std::fs::write(&engine, r#"#!/usr/bin/env python3
import sys, re, time, os
sql = sys.stdin.read()
print('secret-token', file=sys.stderr)
print('secret-token')
total = re.search(r"TO '([^']+)' \(FORMAT CSV, HEADER FALSE", sql).group(1)
path = re.search(r"TO '([^']+)' \(FORMAT CSV, HEADER TRUE", sql).group(1)
open(total, 'w').write('2\n')
with open(path, 'w') as csv:
    csv.write('fsq_place_id,name,country,fsq_category_ids,date_closed\np1,"Cafe\nNorth",CA,"[""restaurant""]",\n')
    csv.flush()
    deadline = time.monotonic() + 30
    while not os.path.exists(os.environ['PROGRESS_ACK']):
        if time.monotonic() > deadline:
            sys.exit('progress acknowledgement timed out')
        time.sleep(0.02)
    csv.write('p2,Cafe South,CA,"[""restaurant""]",\n')
"#).unwrap();
    std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o700)).unwrap();
    let acknowledgement = root.path().join("progress-ack");
    let mut child = Command::new(env!("CARGO_BIN_EXE_ultrafinance"))
        .args(["sources", "--cache-dir"])
        .arg(root.path().join("cache"))
        .arg("--duckdb-cli")
        .arg(&engine)
        .args(["download", "foursquare", "--region", "global"])
        .env("ULTRAFINANCE_FOURSQUARE_TOKEN", "secret-token")
        .env("PROGRESS_ACK", &acknowledgement)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut progress = String::new();
    for line in BufReader::new(child.stderr.take().unwrap()).lines() {
        let line = line.unwrap();
        if line.contains("1/2 rows (50.0%)") {
            std::fs::write(&acknowledgement, "observed").unwrap();
        }
        progress.push_str(&line);
        progress.push('\n');
    }
    let result = child.wait_with_output().unwrap();
    assert!(progress.contains("1/2 rows (50.0%)"), "{progress}");
    assert!(progress.contains("2/2 rows (100.0%)"), "{progress}");
    assert!(!progress.contains("secret-token"));
    assert!(!String::from_utf8_lossy(&result.stdout).contains("secret-token"));
    assert!(!progress.contains('\u{1b}'));
    json(result);
}

#[test]
fn source_import_reconciles_against_catalog_and_batch_with_preview() {
    let store = ultrafinance_core::store::MerchantStore::temporary().unwrap();
    let manual:ultrafinance_core::Merchant=serde_json::from_value(serde_json::json!({"id":"brand","name":"Brand","website":"https://brand.test","logo_url":"https://brand.test/verified.png","logo_source":"manual"})).unwrap();
    store.put(&manual).unwrap();
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("places.csv");
    std::fs::write(
        &input,
        r#"fsq_place_id,name,country,website,fsq_category_ids,date_closed
a,Brand,CA,https://brand.test,"[""restaurant""]",
b,BRAND,CA,https://www.brand.test/outlet,"[""restaurant""]",
"#,
    )
    .unwrap();
    let execute = |preview: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ultrafinance"));
        command
            .args(["sources", "--cache-dir"])
            .arg(root.path().join("cache"))
            .args(["import", "foursquare", "--verbose", "--input"])
            .arg(&input)
            .args(["--region", "ca", "--output"])
            .arg(root.path().join("bundles"))
            .env("ULTRAFINANCE_DATABASE_URL", store.temporary_url())
            .env("TYPESAFE_API_KEY", "invalid-import-must-not-use")
            .env(
                "ULTRAFINANCE_DEDUPE_CHECKPOINT",
                root.path().join("missing-parent/checkpoint.jsonl"),
            );
        if preview {
            command.arg("--dedupe-dry-run");
        }
        let output = command.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("2 available source records; 2 selected"));
        assert!(stderr.contains("100.0% · 2/2"));
        assert!(stderr.contains("matching new merchant identities"));
        assert!(stderr.contains("processing places for linked locations 2/2 (100%)"));
        if preview {
            assert!(stderr.contains("identity preview complete"));
            assert!(!stderr.contains("Import: committed"));
        } else {
            assert!(stderr.contains("writing current Foursquare records 2/2 (100%)"));
            assert!(stderr.contains("Import: committed"));
        }
        json(output)
    };
    let preview = execute(true);
    assert_eq!(preview["dry_run"], true);
    assert_eq!(preview["dedupe"]["candidates"], 2);
    assert_eq!(
        preview["dedupe"]["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g.as_array().unwrap().len() - 1)
            .sum::<usize>(),
        2
    );
    assert!(
        store
            .resolve_source("foursquare", "place:a")
            .unwrap()
            .is_none()
    );
    let applied = execute(false);
    assert_eq!(applied["delta"]["added"], 2);
    assert_eq!(store.stats().unwrap().total, 1);
    for id in ["place:a", "place:b"] {
        assert_eq!(
            store.resolve_source("foursquare", id).unwrap(),
            Some("brand".into())
        );
    }
    assert_eq!(
        store.get("brand").unwrap().unwrap().logo_url,
        manual.logo_url
    );
    assert_eq!(execute(false)["delta"]["unchanged"], 2);
}

#[test]
fn source_import_limit_caps_cached_records_without_truncating_snapshots_or_bundles() {
    let store = ultrafinance_core::store::MerchantStore::temporary().unwrap();
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("places.csv");
    std::fs::write(
        &input,
        r#"fsq_place_id,name,country,website,fsq_category_ids,date_closed
a,Alpha,CA,https://alpha.test,"[""restaurant""]",
b,Beta,CA,https://beta.test,"[""restaurant""]",
c,Gamma,CA,https://gamma.test,"[""restaurant""]",
"#,
    )
    .unwrap();
    let execute = |extra: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ultrafinance"));
        command
            .args(["sources", "--cache-dir"])
            .arg(root.path().join("cache"))
            .args(["import", "foursquare", "--region", "ca", "--output"])
            .arg(root.path().join("bundles"))
            .args(extra)
            .env("ULTRAFINANCE_DATABASE_URL", store.temporary_url());
        let output = command.output().unwrap();
        if !extra.contains(&"--dry-run") {
            let expected = extra
                .windows(2)
                .find(|pair| pair[0] == "--limit")
                .map_or("3", |pair| pair[1]);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains(&format!("3 available source records; {expected} selected")));
            assert!(stderr.contains("Import: committed"));
            assert!(!stderr.contains("looking up existing source identities"));
            assert!(!stderr.contains("matching new merchant identities"));
            assert!(!stderr.contains("reconciling merchant groups"));
            assert!(!stderr.contains("checking/writing source records"));
        }
        json(output)
    };
    let preview = execute(&[
        "--input",
        input.to_str().unwrap(),
        "--limit",
        "2",
        "--dry-run",
    ]);
    assert_eq!(preview["selection"]["available_records"], 3);
    assert_eq!(preview["selection"]["selected_records"], 2);
    assert_eq!(preview["manifest"]["merchant_records"], 3);
    assert!(preview["bundle"].is_null());
    let checkpoint = std::path::Path::new(preview["checkpoint"].as_str().unwrap());
    assert!(!checkpoint.join("knowledge.json").exists());
    assert_eq!(store.stats().unwrap().total, 0);
    assert_eq!(execute(&["--offline", "--limit", "1"])["delta"]["added"], 1);
    assert!(
        store
            .resolve_source("foursquare", "place:b")
            .unwrap()
            .is_none()
    );
    let larger = execute(&["--offline", "--limit", "2"]);
    assert_eq!(larger["delta"]["added"], 1);
    assert_eq!(larger["delta"]["unchanged"], 0);
    assert_eq!(larger["resume"]["previously_committed"], 1);
    assert!(
        store
            .resolve_source("foursquare", "place:c")
            .unwrap()
            .is_none()
    );
    let complete = execute(&["--offline"]);
    assert_eq!(complete["delta"]["added"], 1);
    assert_eq!(complete["delta"]["unchanged"], 0);
    assert_eq!(complete["resume"]["previously_committed"], 2);
    assert_eq!(store.stats().unwrap().total, 3);
}

#[test]
fn foursquare_checkpoint_resume_needs_no_download_cache_or_original_csv() {
    let root = tempfile::tempdir().unwrap();
    let store = ultrafinance_core::store::MerchantStore::temporary().unwrap();
    let input = root.path().join("input.csv");
    std::fs::write(&input, "fsq_place_id,name,country,fsq_category_ids,date_closed\na,Cafe A,CA,\"[\"\"restaurant\"\"]\",\nb,Cafe B,CA,\"[\"\"restaurant\"\"]\",\nc,Cafe C,CA,\"[\"\"restaurant\"\"]\",\n").unwrap();
    let output = root.path().join("out");
    let invoke = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ultrafinance"))
            .args(["sources", "--cache-dir"])
            .arg(root.path().join("cache"))
            .args(args)
            .env("ULTRAFINANCE_DATABASE_URL", store.temporary_url())
            .env_remove("ULTRAFINANCE_FOURSQUARE_TOKEN")
            .output()
            .unwrap()
    };
    let first = json(invoke(&[
        "import",
        "foursquare",
        "--region",
        "ca",
        "--input",
        input.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
        "--limit",
        "1",
        "--chunk-size",
        "1",
    ]));
    std::fs::remove_file(&input).unwrap();
    std::fs::remove_dir_all(root.path().join("cache")).unwrap();
    let resumed = invoke(&[
        "import",
        "foursquare",
        "--region",
        "ca",
        "--resume",
        first["checkpoint"].as_str().unwrap(),
        "--chunk-size",
        "1",
    ]);
    let stderr = String::from_utf8_lossy(&resumed.stderr);
    assert!(stderr.contains("resuming at 1 committed source records"));
    assert!(!stderr.contains("hashing input CSV") && !stderr.contains("Downloading Foursquare"));
    let report = json(resumed);
    assert_eq!(report["resume"]["previously_committed"], 1);
    assert_eq!(report["resume"]["committed_this_run"], 2);
    assert_eq!(report["resume"]["complete"], true);
    assert_eq!(
        store
            .source_records("foursquare", None, 100, 0)
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn foursquare_checkpoint_resume_flags_reject_incompatible_modes() {
    let root = tempfile::tempdir().unwrap();
    for extra in [
        vec!["--offline"],
        vec!["--input", "input.csv"],
        vec!["--dry-run"],
        vec!["--dedupe-dry-run"],
    ] {
        let mut args = vec!["import", "foursquare", "--resume", "checkpoint"];
        args.extend(extra);
        let result = run(root.path(), &args);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("cannot be used with"));
    }
    let result = run(
        root.path(),
        &["import", "merchant-studio", "--resume", "checkpoint"],
    );
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("only for Foursquare"));
}

#[test]
fn initial_load_requires_foursquare_and_conflicts_with_resume() {
    let root = tempfile::tempdir().unwrap();
    let wrong_source = run(
        root.path(),
        &["import", "merchant-studio", "--initial-load", "--dry-run"],
    );
    assert!(!wrong_source.status.success());
    assert!(
        String::from_utf8_lossy(&wrong_source.stderr).contains("supported only for Foursquare")
    );
    let resume = run(
        root.path(),
        &[
            "import",
            "foursquare",
            "--initial-load",
            "--resume",
            "missing",
        ],
    );
    assert!(!resume.status.success());
    assert!(String::from_utf8_lossy(&resume.stderr).contains("cannot be used with"));
}

#[test]
fn merchants_only_requires_foursquare() {
    let root = tempfile::tempdir().unwrap();
    let result = run(
        root.path(),
        &["import", "merchant-studio", "--merchants-only", "--dry-run"],
    );
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("--merchants-only is supported only for Foursquare")
    );
}
