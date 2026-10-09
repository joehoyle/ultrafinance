//! Bounded external sorting preserves stable place order and duplicate validation
//! without holding a regional export's merchants or place-ID map in memory.
use super::*;
use std::{
    cmp::Reverse,
    collections::BinaryHeap,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::PathBuf,
};
const RUN: usize = 5000;
const FAN_IN: usize = 64;
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fingerprint_file(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    let bytes = usize::try_from(file.metadata()?.len())?;
    let mut reader = BufReader::new(crate::import_progress::Reader::new(
        file,
        "hashing input CSV (bytes)",
        bytes,
    ));
    let mut buffer = [0; 65536];
    let mut hash = 0xcbf29ce484222325u64;
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        for byte in &buffer[..n] {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    Ok(format!("fnv1a64:{hash:016x}"))
}
fn row_csv(headers: &csv::StringRecord, row: Option<&csv::StringRecord>) -> Result<String> {
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(headers)?;
    if let Some(row) = row {
        writer.write_record(row)?;
    }
    Ok(String::from_utf8(writer.into_inner()?)?)
}
fn write_prepared(
    headers: &csv::StringRecord,
    rows: &mut Vec<csv::StringRecord>,
    output: &mut impl Write,
    manifest: &mut Manifest,
    fingerprint: &str,
    region: &str,
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(headers)?;
    for row in rows.drain(..) {
        writer.write_record(&row)?;
    }
    let contents = String::from_utf8(writer.into_inner()?)?;
    let (records, skipped) = crate::foursquare::prepare(&contents, None, region)?;
    manifest.skipped_places += skipped;
    for mut record in records {
        record.version = Some(format!("{fingerprint}:{region}:"));
        if manifest.merchant_records > 0 {
            output.write_all(b",")?;
        }
        serde_json::to_writer(&mut *output, &record)?;
        manifest.merchant_records += 1;
        manifest.retained_places += 1;
    }
    Ok(())
}
fn next(reader: &mut BufReader<File>) -> Result<Option<(String, String)>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let (id, _): (String, Vec<String>) = serde_json::from_str(&line)?;
    Ok(Some((id, line)))
}
fn merge(paths: &[PathBuf], output: &Path) -> Result<()> {
    let mut readers: Vec<_> = paths
        .iter()
        .map(|p| Ok(BufReader::new(File::open(p)?)))
        .collect::<Result<_>>()?;
    let mut heap = BinaryHeap::new();
    for (i, r) in readers.iter_mut().enumerate() {
        if let Some((id, line)) = next(r)? {
            heap.push(Reverse((id, line, i)));
        }
    }
    let mut out = BufWriter::new(File::create(output)?);
    while let Some(Reverse((_, line, i))) = heap.pop() {
        out.write_all(line.as_bytes())?;
        if let Some((id, line)) = next(&mut readers[i])? {
            heap.push(Reverse((id, line, i)));
        }
    }
    out.flush()?;
    Ok(())
}
fn run(rows: &mut Vec<(String, Vec<String>)>, path: &Path) -> Result<()> {
    rows.sort();
    let mut writer = BufWriter::new(File::create(path)?);
    for row in rows.drain(..) {
        serde_json::to_writer(&mut writer, &row)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    Ok(())
}
pub(super) fn prepare_file(
    input: &Path,
    directory: &Path,
    region: &str,
) -> Result<(PathBuf, Manifest)> {
    let fingerprint = fingerprint_file(input)?;
    let final_path = bundle_path(
        directory,
        "foursquare",
        adapter_version(Source::Foursquare),
        region,
        &fingerprint,
        None,
    );
    if final_path.exists() {
        let manifest: Manifest =
            serde_json::from_reader(File::open(final_path.join("manifest.json"))?)?;
        if manifest.input_fingerprint != fingerprint
            || manifest.region != region
            || manifest.examples_fingerprint.is_some()
            || manifest.source != "foursquare"
            || manifest.adapter_version != adapter_version(Source::Foursquare)
        {
            bail!("cached dataset metadata does not match its input");
        }
        for name in ["knowledge.json", "development.jsonl", "holdout.jsonl"] {
            if !final_path.join(name).is_file() {
                bail!("incomplete existing dataset at {}", final_path.display());
            }
        }
        eprintln!("Import: reusing prepared foursquare merchant bundle");
        return Ok((final_path, manifest));
    }
    eprintln!("Import: preparing foursquare merchant bundle in bounded disk runs");
    let scratch = Scratch(
        directory
            .join("foursquare")
            .join(format!(".preparing-{}", uuid::Uuid::new_v4())),
    );
    std::fs::create_dir_all(&scratch.0)?;
    let file = File::open(input)?;
    let bytes = usize::try_from(file.metadata()?.len())?;
    let tracked = crate::import_progress::Reader::new(file, "sorting input CSV (bytes)", bytes);
    let mut reader = csv::Reader::from_reader(tracked);
    let headers = reader.headers()?.clone();
    let id_column = headers
        .iter()
        .position(|h| h == "fsq_place_id")
        .context("missing Foursquare CSV column fsq_place_id")?;
    let mut manifest =
        prepare(Source::Foursquare, &row_csv(&headers, None)?, None, region)?.manifest;
    manifest.input_fingerprint = fingerprint.clone();
    let mut rows = Vec::with_capacity(RUN);
    let mut paths = Vec::new();
    let mut total_rows = 0;
    for row in reader.records() {
        let row = row?;
        total_rows += 1;
        let id = row.get(id_column).unwrap_or("").trim().to_owned();
        if id.is_empty() {
            bail!("Foursquare place ID cannot be blank");
        }
        rows.push((id, row.iter().map(str::to_owned).collect()));
        if rows.len() == RUN {
            let path = scratch.0.join(format!("run-{}", paths.len()));
            run(&mut rows, &path)?;
            paths.push(path);
        }
    }
    if !rows.is_empty() {
        let path = scratch.0.join(format!("run-{}", paths.len()));
        run(&mut rows, &path)?;
        paths.push(path);
    }
    let mut pass = 0;
    while paths.len() > 1 {
        let mut merged = Vec::new();
        let mut progress =
            crate::import_progress::Progress::new("merging sorted runs", paths.len());
        let mut processed = 0;
        for (i, chunk) in paths.chunks(FAN_IN).enumerate() {
            let path = scratch.0.join(format!("merge-{pass}-{i}"));
            merge(chunk, &path)?;
            merged.push(path);
            processed += chunk.len();
            progress.advance(processed);
            for old in chunk {
                std::fs::remove_file(old)?;
            }
        }
        progress.finish();
        paths = merged;
        pass += 1;
    }
    let mut output = BufWriter::new(File::create(scratch.0.join("knowledge.json"))?);
    output.write_all(b"[")?;
    let mut previous: Option<(String, Vec<String>)> = None;
    let mut progress = crate::import_progress::Progress::new("preparing place records", total_rows);
    let mut processed = 0;
    let mut prepared_rows = Vec::with_capacity(RUN);
    if let Some(path) = paths.first() {
        for line in BufReader::new(File::open(path)?).lines() {
            let (id, fields): (String, Vec<String>) = serde_json::from_str(&line?)?;
            processed += 1;
            progress.advance(processed);
            if let Some((old_id, old_fields)) = &previous
                && old_id == &id
            {
                if old_fields != &fields {
                    bail!("conflicting Foursquare rows for place ID {id}");
                }
                manifest.skipped_places += 1;
                continue;
            }
            prepared_rows.push(csv::StringRecord::from(fields.clone()));
            if prepared_rows.len() == RUN {
                write_prepared(
                    &headers,
                    &mut prepared_rows,
                    &mut output,
                    &mut manifest,
                    &fingerprint,
                    region,
                )?;
            }
            previous = Some((id, fields));
        }
        std::fs::remove_file(path)?;
    }
    write_prepared(
        &headers,
        &mut prepared_rows,
        &mut output,
        &mut manifest,
        &fingerprint,
        region,
    )?;
    progress.finish();
    output.write_all(b"]")?;
    output.flush()?;
    drop(output);
    for name in ["development.jsonl", "holdout.jsonl"] {
        std::fs::write(scratch.0.join(name), "")?;
    }
    std::fs::write(scratch.0.join("NOTICE.txt"), crate::foursquare::NOTICE)?;
    std::fs::write(scratch.0.join("LICENSE.txt"), crate::foursquare::LICENSE)?;
    std::fs::write(
        scratch.0.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    std::fs::rename(&scratch.0, &final_path)?;
    Ok((final_path, manifest))
}

#[cfg(test)]
mod tests {
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
    fn streaming_preparation_matches_adapter_and_reuses_reviewed_bundle() -> Result<()> {
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
}
