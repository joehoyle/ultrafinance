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
