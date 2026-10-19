use super::*;
use serde_json::json;

#[test]
fn resume_accepts_optional_path_and_preserves_conflicts() {
    use clap::Parser;
    for tail in [vec!["--resume"], vec!["--resume", "saved.streaming"]] {
        let mut args = vec!["ultrafinance", "sources", "import", "foursquare"];
        args.extend(tail);
        assert!(crate::Cli::try_parse_from(&args).is_ok());
        for conflict in [
            "--offline",
            "--dry-run",
            "--dedupe-dry-run",
            "--initial-load",
        ] {
            let mut conflicting = args.clone();
            conflicting.push(conflict);
            assert!(crate::Cli::try_parse_from(conflicting).is_err());
        }
    }
    let parsed = crate::Cli::try_parse_from([
        "ultrafinance",
        "sources",
        "import",
        "foursquare",
        "--resume",
        "--region",
        "ca",
    ])
    .unwrap();
    if let crate::Command::Sources(args) = parsed.command {
        assert!(matches!(
            args.command,
            super::super::Command::Import {
                resume: Some(None),
                ..
            }
        ));
    } else {
        panic!("expected sources command");
    }
}

#[test]
fn discovery_excludes_completed_initial_preview_and_other_region_or_mode() {
    let root = tempfile::tempdir().unwrap();
    let base = json!({
        "format": 1, "stage": "Preparing", "total_rows": 100,
        "manifest": {"source": "foursquare", "region": "global"},
        "application": {"complete": false, "merchants_only": false, "processed": 42}
    });
    for name in [
        "active", "complete", "initial", "preview", "region", "mode", "sorting",
    ] {
        let mut value = base.clone();
        match name {
            "complete" => value["application"]["complete"] = json!(true),
            "initial" | "preview" => value["application"] = Value::Null,
            "region" => value["manifest"]["region"] = json!("ca"),
            "mode" => value["application"]["merchants_only"] = json!(true),
            "sorting" => value["stage"] = json!("Sorting"),
            _ => {}
        }
        let directory = root.path().join("foursquare").join(name);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("checkpoint.json"), value.to_string()).unwrap();
    }
    let candidates = discover(root.path(), "global", false).unwrap();
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].path.ends_with("active"));
    assert_eq!(candidates[0].processed, 42);
    assert_eq!(discover(root.path(), "ca", false).unwrap().len(), 1);
    assert_eq!(discover(root.path(), "global", true).unwrap().len(), 1);
    assert!(
        discover(&root.path().join("missing"), "global", false)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn checkpoint_label_includes_path_and_saved_progress() {
    let candidate = Candidate {
        path: "saved.streaming".into(),
        processed: 13_700_000,
        total: 101_852_011,
    };
    assert_eq!(
        candidate.to_string(),
        "saved.streaming — 13700000/101852011 rows committed"
    );
}
