use super::*;
fn input(root: &Path) -> Result<PathBuf> {
    let path = root.join("input.csv");
    let mut writer = csv::Writer::from_path(&path)?;
    writer.write_record([
        "fsq_place_id",
        "name",
        "country",
        "address",
        "fsq_category_ids",
        "date_closed",
    ])?;
    for i in (0..5).rev() {
        writer.write_record([
            i.to_string(),
            "Same Cafe".into(),
            "CA".into(),
            format!("{i} Main St"),
            "[\"restaurant\"]".into(),
            "".into(),
        ])?;
    }
    writer.write_record(["0", "Same Cafe", "CA", "0 Main St", "[\"restaurant\"]", ""])?;
    writer.flush()?;
    Ok(path)
}
fn options(
    store: Option<&MerchantStore>,
    dry_run: bool,
    limit: Option<u32>,
) -> FoursquareImportOptions {
    FoursquareImportOptions {
        initial_load: false,
        database_url: store.map(|s| s.temporary_url().to_owned()),
        limit,
        dry_run,
        dedupe: crate::dedupe::ImportOptions {
            dry_run: false,
            chunk_size: 2,
        },
    }
}
#[test]
fn dry_run_creates_no_bundle_and_validates_duplicates() -> Result<()> {
    let root = Scratch(
        std::env::temp_dir().join(format!("ultra-fsq-direct-preview-{}", uuid::Uuid::new_v4())),
    );
    std::fs::create_dir_all(&root.0)?;
    let input = input(&root.0)?;
    let report = import_file(
        &input,
        &root.0.join("out"),
        "ca",
        options(None, true, Some(2)),
    )?;
    assert_eq!(report["manifest"]["merchant_records"], 5);
    assert_eq!(report["manifest"]["skipped_places"], 1);
    assert_eq!(report["selection"]["selected_records"], 2);
    assert!(report["bundle"].is_null());
    let path = Path::new(report["checkpoint"].as_str().unwrap());
    assert!(!path.join("knowledge.json").exists());
    let report2 = import_file(&input, &root.0.join("out"), "ca", options(None, true, None))?;
    assert_eq!(report2["selection"]["selected_records"], 5);
    assert_eq!(report["checkpoint"], report2["checkpoint"]);
    let mut contents = std::fs::read_to_string(&input)?;
    contents.push_str("0,Different Cafe,CA,0 Main St,\"[\"\"restaurant\"\"]\",\n");
    std::fs::write(&input, contents)?;
    assert!(import_file(&input, &root.0.join("out"), "ca", options(None, true, None)).is_err());
    Ok(())
}
#[test]
fn committed_chunks_survive_cursor_failure_and_resume_without_duplicate_outlets() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let root = Scratch(
        std::env::temp_dir().join(format!("ultra-fsq-direct-resume-{}", uuid::Uuid::new_v4())),
    );
    std::fs::create_dir_all(&root.0)?;
    let input = input(&root.0)?;
    let output = root.0.join("out");
    // Final sort, counts, application start, and first committed cursor succeed.
    // The second DB commit succeeds but saving its cursor fails.
    CHECKPOINT_FAILURE.with(|f| *f.borrow_mut() = Some((Stage::Preparing, 4)));
    assert!(import_file(&input, &output, "ca", options(Some(&store), false, None)).is_err());
    let base = bundle_path(
        &output,
        "foursquare",
        adapter_version(Source::Foursquare),
        "ca",
        &fingerprint_file(&input)?,
        None,
    );
    let scratch = base.with_extension("streaming");
    let saved: Checkpoint = serde_json::from_reader(File::open(scratch.join("checkpoint.json"))?)?;
    assert_eq!(saved.application.as_ref().unwrap().selected, 2);
    assert_eq!(store.source_records("foursquare", None, 100, 0)?.len(), 4);
    assert_eq!(store.stats()?.total, 1);
    let merchant = store.resolve_source("foursquare", "place:0")?.unwrap();
    let first_locations = store.location_sources(&merchant)?;
    assert_eq!(first_locations.len(), 4);
    let other = MerchantStore::temporary()?;
    let error = import_file(&input, &output, "ca", options(Some(&other), false, None))
        .err()
        .context("expected database mismatch")?;
    assert!(error.to_string().contains("different database"));
    assert_eq!(other.stats()?.total, 0);
    let report = import_file(&input, &output, "ca", options(Some(&store), false, None))?;
    assert_eq!(report["resume"]["previously_committed"], 2);
    assert_eq!(report["resume"]["committed_this_run"], 3);
    assert_eq!(report["delta"]["added"], 1);
    assert_eq!(report["delta"]["unchanged"], 2);
    assert_eq!(report["resume"]["complete"], true);
    assert_eq!(store.source_records("foursquare", None, 100, 0)?.len(), 5);
    assert_eq!(store.stats()?.total, 1);
    let locations = store.location_sources(&merchant)?;
    assert_eq!(locations.len(), 5);
    for before in first_locations {
        assert!(
            locations
                .iter()
                .any(|after| before.location.id == after.location.id)
        );
    }
    assert!(!scratch.join("knowledge.json").exists());
    assert!(
        !std::fs::read_dir(&scratch)?
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("merge-"))
    );
    let repeated = import_file(&input, &output, "ca", options(Some(&store), false, None))?;
    assert_eq!(repeated["delta"]["added"], 0);
    assert_eq!(repeated["delta"]["unchanged"], 5);
    assert_eq!(store.location_sources(&merchant)?.len(), 5);
    Ok(())
}
#[test]
fn limited_import_continues_at_offset_and_preview_rolls_back_across_chunks() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let root = Scratch(
        std::env::temp_dir().join(format!("ultra-fsq-direct-limit-{}", uuid::Uuid::new_v4())),
    );
    std::fs::create_dir_all(&root.0)?;
    let input = input(&root.0)?;
    let output = root.0.join("out");
    let mut preview = options(Some(&store), false, None);
    preview.dedupe.dry_run = true;
    let report = import_file(&input, &output, "ca", preview)?;
    assert_eq!(report["dedupe"]["candidates"], 4);
    assert_eq!(store.stats()?.total, 0);
    let first = import_file(&input, &output, "ca", options(Some(&store), false, Some(1)))?;
    assert_eq!(first["resume"]["complete"], false);
    assert_eq!(first["delta"]["added"], 1);
    let same = import_file(&input, &output, "ca", options(Some(&store), false, Some(1)))?;
    assert_eq!(same["resume"]["committed_this_run"], 0);
    assert!(import_file(&input, &output, "ca", options(Some(&store), false, Some(0))).is_err());
    let rest = import_file(&input, &output, "ca", options(Some(&store), false, None))?;
    assert_eq!(rest["resume"]["previously_committed"], 1);
    assert_eq!(rest["delta"]["added"], 4);
    assert_eq!(store.source_records("foursquare", None, 100, 0)?.len(), 5);
    assert_eq!(store.stats()?.total, 1);
    Ok(())
}

#[test]
fn consumed_sorted_parts_are_deleted_after_commit_and_resume_skips_them() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let root = Scratch(
        std::env::temp_dir().join(format!("ultra-fsq-direct-parts-{}", uuid::Uuid::new_v4())),
    );
    std::fs::create_dir_all(&root.0)?;
    let input = root.0.join("input.csv");
    let mut writer = csv::Writer::from_path(&input)?;
    writer.write_record([
        "fsq_place_id",
        "name",
        "country",
        "fsq_category_ids",
        "date_closed",
    ])?;
    for i in (0..PART_ROWS + 2).rev() {
        writer.write_record([
            format!("{i:08}"),
            "Same Cafe".into(),
            "CA".into(),
            "[\"restaurant\"]".into(),
            "".into(),
        ])?;
    }
    writer.flush()?;
    let output = root.0.join("out");
    let mut first_options = options(Some(&store), false, Some(PART_ROWS as u32));
    first_options.dedupe.chunk_size = RUN;
    let first = import_file(&input, &output, "ca", first_options)?;
    let scratch = Path::new(first["checkpoint"].as_str().unwrap());
    let saved: Checkpoint = serde_json::from_reader(File::open(scratch.join("checkpoint.json"))?)?;
    assert_eq!(saved.paths.len(), 1);
    assert_eq!(saved.application.as_ref().unwrap().offset, 0);
    assert_eq!(saved.application.as_ref().unwrap().selected, PART_ROWS);
    assert_eq!(
        std::fs::read_dir(scratch)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("merge-"))
            .count(),
        1
    );
    let second = import_file(&input, &output, "ca", options(Some(&store), false, None))?;
    assert_eq!(second["resume"]["previously_committed"], PART_ROWS);
    assert_eq!(second["delta"]["added"], 2);
    assert_eq!(second["delta"]["unchanged"], 0);
    assert_eq!(store.stats()?.total, 1);
    Ok(())
}

#[test]
fn failing_generated_database_chunk_rolls_back_its_own_writes() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let records = crate::foursquare::prepare("fsq_place_id,name,country,fsq_category_ids,date_closed\na,Cafe,CA,\"[\"\"restaurant\"\"]\",\n", "ca")?.0;
    let error = store.reconcile_generated(1, false, 2, move |consume| {
        consume(records)?;
        bail!("simulated database chunk failure")
    });
    assert!(error.is_err());
    assert_eq!(store.stats()?.total, 0);
    assert!(store.resolve_source("foursquare", "place:a")?.is_none());
    Ok(())
}

#[test]
fn resume_past_weak_outlet_alias_preserves_merchants_and_valid_outlets() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let root = Scratch(
        std::env::temp_dir().join(format!("ultra-fsq-weak-outlet-{}", uuid::Uuid::new_v4())),
    );
    std::fs::create_dir_all(&root.0)?;
    let input = root.0.join("input.csv");
    let mut writer = csv::Writer::from_path(&input)?;
    writer.write_record([
        "fsq_place_id",
        "name",
        "country",
        "address",
        "fsq_category_ids",
        "date_closed",
    ])?;
    for (id, name, address) in [
        ("a", "Before Cafe", "1 Main St"),
        ("b", "+1", "\u{e332}"),
        ("c", "After Cafe", "3 Main St"),
    ] {
        writer.write_record([id, name, "CA", address, "[\"restaurant\"]", ""])?;
    }
    writer.flush()?;
    let output = root.0.join("out");
    let first = import_file(&input, &output, "ca", options(Some(&store), false, Some(1)))?;
    assert_eq!(first["delta"]["added"], 1);
    assert_eq!(first["resume"]["complete"], false);
    // A previously validated checkpoint remains usable: merchant counts and
    // the input/adapter version do not change when weak outlet evidence is skipped.
    let resumed = import_file(&input, &output, "ca", options(Some(&store), false, None))?;
    assert_eq!(resumed["resume"]["previously_committed"], 1);
    assert_eq!(resumed["delta"]["added"], 2);
    assert_eq!(resumed["resume"]["complete"], true);
    assert_eq!(store.source_records("foursquare", None, 100, 0)?.len(), 3);
    let weak = store.resolve_source("foursquare", "place:b")?.unwrap();
    assert!(store.location_sources(&weak)?.is_empty());
    for id in ["place:a", "place:c"] {
        let merchant = store.resolve_source("foursquare", id)?.unwrap();
        assert_eq!(store.location_sources(&merchant)?.len(), 1);
    }
    Ok(())
}

#[test]
fn checkpoint_only_resume_survives_deleted_csv_and_validates_target_and_files() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let root =
        Scratch(std::env::temp_dir().join(format!("ultra-fsq-no-csv-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(&root.0)?;
    let input = input(&root.0)?;
    let first = import_file(
        &input,
        &root.0.join("out"),
        "ca",
        options(Some(&store), false, Some(2)),
    )?;
    let scratch = PathBuf::from(first["checkpoint"].as_str().unwrap());
    std::fs::remove_file(input)?;
    let before = std::fs::read(scratch.join("checkpoint.json"))?;
    let other = MerchantStore::temporary()?;
    let error = resume_import(&scratch, "ca", options(Some(&other), false, None)).unwrap_err();
    assert!(error.to_string().contains("different database"));
    assert_eq!(other.stats()?.total, 0);
    assert!(resume_import(&scratch, "global", options(Some(&store), false, None)).is_err());
    assert!(resume_import(&scratch, "ca", options(Some(&store), true, None)).is_err());
    let saved: Checkpoint = serde_json::from_slice(&before)?;
    let part = scratch.join(&saved.paths[0]);
    let hidden = scratch.join("saved-part");
    std::fs::rename(&part, &hidden)?;
    assert!(resume_import(&scratch, "ca", options(Some(&store), false, None)).is_err());
    assert_eq!(std::fs::read(scratch.join("checkpoint.json"))?, before);
    std::fs::rename(hidden, part)?;
    let report = resume_import(
        &scratch.join("checkpoint.json"),
        "ca",
        options(Some(&store), false, None),
    )?;
    assert_eq!(report["resume"]["previously_committed"], 2);
    assert_eq!(report["resume"]["committed_this_run"], 3);
    assert_eq!(report["resume"]["complete"], true);
    assert_eq!(store.source_records("foursquare", None, 100, 0)?.len(), 5);
    assert_eq!(store.stats()?.total, 1);
    let repeated = resume_import(&scratch, "ca", options(Some(&store), false, None))?;
    assert_eq!(repeated["resume"]["committed_this_run"], 0);
    assert_eq!(repeated["delta"]["added"], 0);
    Ok(())
}

#[test]
fn checkpoint_only_resume_rejects_incomplete_sorting_or_validation() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let root = Scratch(std::env::temp_dir().join(format!(
        "ultra-fsq-resume-validation-{}",
        uuid::Uuid::new_v4()
    )));
    std::fs::create_dir_all(&root.0)?;
    let input = input(&root.0)?;
    let first = import_file(
        &input,
        &root.0.join("out"),
        "ca",
        options(Some(&store), false, Some(1)),
    )?;
    let scratch = PathBuf::from(first["checkpoint"].as_str().unwrap());
    std::fs::remove_file(input)?;
    let before = std::fs::read(scratch.join("checkpoint.json"))?;
    for field in ["stage", "counts_ready", "application"] {
        let mut modified: Value = serde_json::from_slice(&before)?;
        modified[field] = match field {
            "stage" => json!("Sorting"),
            "counts_ready" => json!(false),
            _ => Value::Null,
        };
        std::fs::write(
            scratch.join("checkpoint.json"),
            serde_json::to_vec(&modified)?,
        )?;
        let error = resume_import(&scratch, "ca", options(Some(&store), false, None)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("validated Foursquare import checkpoint")
        );
        assert_eq!(store.source_records("foursquare", None, 100, 0)?.len(), 1);
    }
    Ok(())
}

#[test]
fn initial_load_preparation_preview_and_commit_share_retryable_input() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let root =
        Scratch(std::env::temp_dir().join(format!("ultra-fsq-initial-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(&root.0)?;
    let file = input(&root.0)?;
    let out = root.0.join("out");
    let mut config = options(None, true, Some(3));
    config.initial_load = true;
    let prepared = import_file(&file, &out, "ca", config)?;
    assert_eq!(prepared["selection"]["selected_records"], 3);
    assert_eq!(prepared["initial_load"], true);
    let mut config = options(Some(&store), false, Some(3));
    config.initial_load = true;
    config.dedupe.dry_run = true;
    let preview = import_file(&file, &out, "ca", config)?;
    assert_eq!(preview["checkpoint"], prepared["checkpoint"]);
    assert_eq!(store.stats()?.total, 0);
    let mut config = options(Some(&store), false, Some(3));
    config.initial_load = true;
    let committed = import_file(&file, &out, "ca", config)?;
    assert_eq!(committed["delta"]["added"], 3);
    assert_eq!(store.source_records("foursquare", None, 100, 0)?.len(), 3);
    let checkpoint: Checkpoint = serde_json::from_reader(File::open(
        Path::new(committed["checkpoint"].as_str().unwrap()).join("checkpoint.json"),
    )?)?;
    assert!(checkpoint.application.is_none());
    let mut config = options(Some(&store), false, None);
    config.initial_load = true;
    assert!(
        import_file(&file, &out, "ca", config)
            .unwrap_err()
            .to_string()
            .contains("empty catalog")
    );
    Ok(())
}
