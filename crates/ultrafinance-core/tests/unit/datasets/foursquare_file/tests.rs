use super::*;
#[test]
#[ignore = "comparative Foursquare preparation throughput benchmark"]
fn benchmark_streaming_preparation() -> Result<()> {
    let input = PathBuf::from(std::env::var("ULTRAFINANCE_IMPORT_BENCH_INPUT")?);
    let repeats = std::env::var("ULTRAFINANCE_IMPORT_BENCH_REPEATS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(3);
    let root = Scratch(
        std::env::temp_dir().join(format!("ultra-prep-throughput-{}", uuid::Uuid::new_v4())),
    );
    std::fs::create_dir(&root.0)?;
    for repeat in 0..repeats {
        for streaming in if repeat % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let output = root.0.join(format!("{repeat}-{streaming}"));
            let start = std::time::Instant::now();
            let manifest = if streaming {
                prepare_file(&input, &output, "ca")?.1
            } else {
                let contents = std::fs::read_to_string(&input)?;
                let bundle = prepare(Source::Foursquare, &contents, None, "ca")?;
                bundle.save(&output)?;
                bundle.manifest
            };
            let seconds = start.elapsed().as_secs_f64();
            eprintln!(
                "PREPARATION_BENCH strategy={} repeat={repeat} records={} seconds={seconds:.6} records_per_second={:.1}",
                if streaming { "streaming" } else { "in_memory" },
                manifest.merchant_records,
                manifest.merchant_records as f64 / seconds
            );
        }
    }
    Ok(())
}

#[test]
fn streaming_preparation_matches_adapter_and_reuses_cached_bundle() -> Result<()> {
    let root = std::env::temp_dir().join(format!("ultra-fsq-stream-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root)?;
    let input = root.join("input.csv");
    let contents = "fsq_place_id,name,country,website,fsq_category_ids,date_closed\nb,Starbucks,CA,,\"[\"\"restaurant\"\"]\",\na,Starbucks,CA,,\"[\"\"restaurant\"\"]\",\nb,Starbucks,CA,,\"[\"\"restaurant\"\"]\",\n";
    std::fs::write(&input, contents)?;
    let expected = prepare(Source::Foursquare, contents, None, "ca")?;
    let (path, manifest) = prepare_file(&input, &root, "ca")?;
    assert_eq!(
        serde_json::to_value(&manifest)?,
        serde_json::to_value(&expected.manifest)?
    );
    let records: Vec<SourceRecord> =
        serde_json::from_reader(File::open(path.join("knowledge.json"))?)?;
    assert_eq!(
        serde_json::to_value(&records)?,
        serde_json::to_value(&expected.records)?
    );
    assert_eq!(
        count_records(&path.join("knowledge.json"))?,
        manifest.merchant_records
    );
    std::fs::write(path.join("knowledge.json"), "[]")?;
    assert_eq!(count_records(&path.join("knowledge.json"))?, 0);
    assert_eq!(prepare_file(&input, &root, "ca")?.0, path);
    assert_eq!(std::fs::read_to_string(path.join("knowledge.json"))?, "[]");
    std::fs::write(
        &input,
        contents.replace("b,Starbucks,CA", "b,Other,CA").replacen(
            "b,Other,CA",
            "b,Starbucks,CA",
            1,
        ),
    )?;
    assert!(prepare_file(&input, &root, "ca").is_err());
    std::fs::remove_dir_all(root)?;
    Ok(())
}
#[test]
fn preparation_crosses_disk_runs_with_stable_ids() -> Result<()> {
    let root = std::env::temp_dir().join(format!("ultra-fsq-runs-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root)?;
    let input = root.join("input.csv");
    let mut writer = csv::Writer::from_path(&input)?;
    writer.write_record([
        "fsq_place_id",
        "name",
        "country",
        "website",
        "fsq_category_ids",
        "date_closed",
    ])?;
    for i in (0..(RUN + 1)).rev() {
        writer.write_record([
            format!("{i:08}"),
            "Starbucks".into(),
            "CA".into(),
            "".into(),
            "[\"restaurant\"]".into(),
            "".into(),
        ])?;
    }
    writer.write_record(["00000000", "Starbucks", "CA", "", "[\"restaurant\"]", ""])?;
    writer.flush()?;
    let (path, manifest) = prepare_file(&input, &root, "ca")?;
    assert_eq!(manifest.merchant_records, RUN + 1);
    assert_eq!(manifest.skipped_places, 1);
    let (records, total) = read_selected_records(&path.join("knowledge.json"), Some(1))?;
    assert_eq!(total, RUN + 1);
    assert_eq!(records[0].external_id, "place:00000000");
    std::fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn invalid_country_after_a_prepared_batch_does_not_abort_the_bundle() -> Result<()> {
    let root =
        Scratch(std::env::temp_dir().join(format!("ultra-fsq-country-{}", uuid::Uuid::new_v4())));
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
    for index in 0..5001 {
        writer.write_record([
            format!("a{index:05}"),
            "Cafe".into(),
            "CA".into(),
            "[\"restaurant\"]".into(),
            "".into(),
        ])?;
    }
    writer.write_record(["z-invalid", "Unknown country", "", "[\"restaurant\"]", ""])?;
    writer.flush()?;
    let (path, manifest) = prepare_file(&input, &root.0, "global")?;
    assert_eq!(manifest.retained_places, 5001);
    assert_eq!(manifest.merchant_records, 5001);
    assert_eq!(manifest.skipped_places, 1);
    assert_eq!(count_records(&path.join("knowledge.json"))?, 5001);
    let records: Vec<SourceRecord> =
        serde_json::from_reader(File::open(path.join("knowledge.json"))?)?;
    assert!(
        !records
            .iter()
            .any(|record| record.external_id == "place:z-invalid")
    );
    Ok(())
}

fn resumable_input(root: &Path, rows: usize) -> Result<PathBuf> {
    let input = root.join("input.csv");
    let mut writer = csv::Writer::from_path(&input)?;
    writer.write_record([
        "fsq_place_id",
        "name",
        "country",
        "fsq_category_ids",
        "date_closed",
    ])?;
    for i in (0..rows).rev() {
        // A multiline value checks that sorting resumes at a CSV record boundary.
        writer.write_record([
            format!("{i:08}"),
            "Cafe\nNorth".into(),
            "CA".into(),
            "[\"restaurant\"]".into(),
            "".into(),
        ])?;
    }
    // This duplicate falls at a prepared-part boundary after sorting.
    writer.write_record([
        format!("{:08}", (PART_ROWS - 1).min(rows - 1)),
        "Cafe\nNorth".into(),
        "CA".into(),
        "[\"restaurant\"]".into(),
        "".into(),
    ])?;
    writer.flush()?;
    Ok(input)
}
fn preparation_paths(input: &Path, output: &Path) -> Result<(PathBuf, PathBuf)> {
    let final_path = bundle_path(
        output,
        "foursquare",
        adapter_version(Source::Foursquare),
        "ca",
        &fingerprint_file(input)?,
        None,
    );
    let scratch = final_path.with_extension("preparing");
    Ok((final_path, scratch))
}
#[test]
fn preparation_resumes_after_disk_full_and_reclaims_sorted_parts() -> Result<()> {
    let root =
        Scratch(std::env::temp_dir().join(format!("ultra-fsq-resume-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(&root.0)?;
    let input = resumable_input(&root.0, PART_ROWS * 2 + 1)?;
    let output = root.0.join("output");
    CHECKPOINT_FAILURE.with(|failure| *failure.borrow_mut() = Some((Stage::Preparing, 2)));
    let error = prepare_file(&input, &output, "ca")
        .err()
        .context("expected disk failure")?;
    assert!(error.to_string().contains("No space left"));
    let (final_path, scratch) = preparation_paths(&input, &output)?;
    assert!(!final_path.exists());
    let checkpoint: Checkpoint =
        serde_json::from_reader(File::open(scratch.join("checkpoint.json"))?)?;
    assert_eq!(checkpoint.stage, Stage::Preparing);
    assert_eq!(checkpoint.processed, PART_ROWS);
    assert!(std::fs::metadata(scratch.join("knowledge.json"))?.len() > checkpoint.output_bytes);
    let existing_runs: Vec<_> = std::fs::read_dir(&scratch)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("merge-"))
        .collect();
    // The completed first part was deleted; an unsaved second part remains.
    assert_eq!(existing_runs.len(), checkpoint.paths.len());
    for name in &checkpoint.paths {
        assert!(scratch.join(name).exists());
    }
    let (path, manifest) = prepare_file(&input, &output, "ca")?;
    assert_eq!(path, final_path);
    assert_eq!(manifest.merchant_records, PART_ROWS * 2 + 1);
    assert_eq!(manifest.skipped_places, 1);
    assert!(!scratch.exists());
    assert!(!path.join("checkpoint.json").exists());
    assert!(
        !std::fs::read_dir(&path)?
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("merge-"))
    );
    let (fresh, fresh_manifest) = prepare_file(&input, &root.0.join("fresh"), "ca")?;
    assert_eq!(
        serde_json::to_value(manifest)?,
        serde_json::to_value(fresh_manifest)?
    );
    assert_eq!(
        std::fs::read(path.join("knowledge.json"))?,
        std::fs::read(fresh.join("knowledge.json"))?
    );
    Ok(())
}
#[test]
fn sorting_resume_uses_saved_csv_offset_and_cleans_uncommitted_runs() -> Result<()> {
    let root = Scratch(
        std::env::temp_dir().join(format!("ultra-fsq-sort-resume-{}", uuid::Uuid::new_v4())),
    );
    std::fs::create_dir_all(&root.0)?;
    let input = resumable_input(&root.0, RUN * 5)?;
    let output = root.0.join("output");
    CHECKPOINT_FAILURE.with(|failure| *failure.borrow_mut() = Some((Stage::Sorting, 2)));
    assert!(prepare_file(&input, &output, "ca").is_err());
    let (_, scratch) = preparation_paths(&input, &output)?;
    let checkpoint: Checkpoint =
        serde_json::from_reader(File::open(scratch.join("checkpoint.json"))?)?;
    assert_eq!(checkpoint.stage, Stage::Sorting);
    assert_eq!(checkpoint.total_rows, RUN * SORT_CHECKPOINT_RUNS);
    assert!(checkpoint.input_offset > 0);
    let (path, manifest) = prepare_file(&input, &output, "ca")?;
    assert_eq!(manifest.merchant_records, RUN * 5);
    assert_eq!(manifest.skipped_places, 1);
    let fresh = prepare_file(&input, &root.0.join("fresh"), "ca")?.0;
    assert_eq!(
        std::fs::read(path.join("knowledge.json"))?,
        std::fs::read(fresh.join("knowledge.json"))?
    );
    Ok(())
}
#[test]
fn merging_resume_keeps_inputs_until_replacement_is_checkpointed() -> Result<()> {
    let root = Scratch(
        std::env::temp_dir().join(format!("ultra-fsq-merge-resume-{}", uuid::Uuid::new_v4())),
    );
    std::fs::create_dir_all(&root.0)?;
    let input = resumable_input(&root.0, FAN_IN + 1)?;
    let output = root.0.join("output");
    // Stop after sorting has completed and its checkpoint was saved.
    CHECKPOINT_FAILURE.with(|failure| *failure.borrow_mut() = Some((Stage::Preparing, 0)));
    assert!(prepare_file(&input, &output, "ca").is_err());
    let (_, scratch) = preparation_paths(&input, &output)?;
    let mut checkpoint: Checkpoint =
        serde_json::from_reader(File::open(scratch.join("checkpoint.json"))?)?;
    assert_eq!(checkpoint.stage, Stage::Merging);
    clean_runs(&scratch, &checkpoint)?;
    // Split this small fixture into enough runs to exercise an intermediate merge.
    let sorted = std::fs::read_to_string(scratch.join(&checkpoint.paths[0]))?;
    let original = checkpoint.paths.remove(0);
    for (i, line) in sorted.lines().enumerate() {
        let name = format!("run-fixture-{i}");
        std::fs::write(scratch.join(&name), format!("{line}\n"))?;
        checkpoint.paths.push(name);
    }
    checkpoint.save(&scratch)?;
    std::fs::remove_file(scratch.join(original))?;
    CHECKPOINT_FAILURE.with(|failure| *failure.borrow_mut() = Some((Stage::Merging, 0)));
    assert!(prepare_file(&input, &output, "ca").is_err());
    for name in &checkpoint.paths {
        assert!(scratch.join(name).is_file());
    }
    let (path, manifest) = prepare_file(&input, &output, "ca")?;
    assert_eq!(manifest.merchant_records, FAN_IN + 1);
    assert_eq!(manifest.skipped_places, 1);
    assert_eq!(count_records(&path.join("knowledge.json"))?, FAN_IN + 1);
    Ok(())
}

#[test]
fn incomplete_checkpoint_and_concurrent_preparation_are_rejected() -> Result<()> {
    let root = Scratch(
        std::env::temp_dir().join(format!("ultra-fsq-invalid-resume-{}", uuid::Uuid::new_v4())),
    );
    std::fs::create_dir_all(&root.0)?;
    let input = resumable_input(&root.0, 2)?;
    let output = root.0.join("output");
    CHECKPOINT_FAILURE.with(|failure| *failure.borrow_mut() = Some((Stage::Preparing, 0)));
    assert!(prepare_file(&input, &output, "ca").is_err());
    let (final_path, scratch) = preparation_paths(&input, &output)?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(final_path.with_extension("prepare.lock"))?;
    lock.try_lock()?;
    assert!(
        prepare_file(&input, &output, "ca")
            .err()
            .context("expected lock error")?
            .to_string()
            .contains("another process")
    );
    drop(lock);
    let checkpoint: Checkpoint =
        serde_json::from_reader(File::open(scratch.join("checkpoint.json"))?)?;
    std::fs::remove_file(scratch.join(&checkpoint.paths[0]))?;
    assert!(
        prepare_file(&input, &output, "ca")
            .err()
            .context("expected missing run error")?
            .to_string()
            .contains("missing or invalid")
    );
    assert!(!final_path.exists());
    Ok(())
}
