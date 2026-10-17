//! Bounded external sorting preserves stable place order and duplicate validation
//! without holding a regional export's merchants or place-ID map in memory.
use super::*;
use std::{
    cmp::Reverse,
    collections::BinaryHeap,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Read, Seek, Write},
    path::PathBuf,
};
const RUN: usize = 5000;
const FAN_IN: usize = 64;
#[cfg(not(test))]
const SORT_CHECKPOINT_RUNS: usize = FAN_IN;
#[cfg(test)]
const SORT_CHECKPOINT_RUNS: usize = 2;
// Bound the overlap between sorted input and the much larger prepared JSON.
#[cfg(not(test))]
const PART_ROWS: usize = 100_000;
#[cfg(test)]
const PART_ROWS: usize = RUN;

#[derive(Serialize, Deserialize, PartialEq, Debug)]
enum Stage {
    Sorting,
    Merging,
    Preparing,
}
#[derive(Serialize, Deserialize)]
struct Checkpoint {
    format: u32,
    stage: Stage,
    headers: Vec<String>,
    manifest: Manifest,
    // Names relative to the scratch directory, in preparation order.
    paths: Vec<String>,
    input_offset: u64,
    total_rows: usize,
    processed: usize,
    previous: Option<(String, Vec<String>)>,
    output_bytes: u64,
    #[serde(default)]
    counts_ready: bool,
    #[serde(default)]
    application: Option<streaming::ApplyCheckpoint>,
}
impl Checkpoint {
    fn save(&self, directory: &Path) -> Result<()> {
        #[cfg(test)]
        CHECKPOINT_FAILURE.with(|failure| -> Result<()> {
            let mut failure = failure.borrow_mut();
            if let Some((stage, remaining)) = failure.as_mut()
                && *stage == self.stage
            {
                if *remaining == 0 {
                    *failure = None;
                    return Err(std::io::Error::from_raw_os_error(28).into());
                }
                *remaining -= 1;
            }
            Ok(())
        })?;
        let mut file = File::create(directory.join("checkpoint.tmp"))?;
        serde_json::to_writer(&mut file, self)?;
        file.sync_all()?;
        std::fs::rename(
            directory.join("checkpoint.tmp"),
            directory.join("checkpoint.json"),
        )?;
        File::open(directory)?.sync_all()?;
        Ok(())
    }
}
#[cfg(test)]
thread_local! {
    static CHECKPOINT_FAILURE: std::cell::RefCell<Option<(Stage, usize)>> = const { std::cell::RefCell::new(None) };
}
// A crash may leave outputs written before a checkpoint, or inputs whose
// deletion followed it. Only checkpoint-referenced runs remain live.
fn clean_runs(directory: &Path, checkpoint: &Checkpoint) -> Result<()> {
    let live: std::collections::HashSet<_> = checkpoint.paths.iter().map(String::as_str).collect();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if (name.starts_with("run-") || name.starts_with("merge-")) && !live.contains(name.as_ref())
        {
            std::fs::remove_file(entry.path())?;
        }
    }
    for name in &checkpoint.paths {
        // Reject malformed checkpoint paths before opening or deleting files.
        if Path::new(name).components().count() != 1
            || !(name.starts_with("run-") || name.starts_with("merge-"))
            || !directory.join(name).is_file()
        {
            bail!("missing or invalid checkpoint run {name}");
        }
    }
    Ok(())
}
#[cfg(test)]
struct Scratch(PathBuf);
#[cfg(test)]
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
    let (records, skipped) = crate::foursquare::prepare(&contents, region)?;
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
// Intermediate merges produce one run; the final merge produces small,
// globally ordered parts so preparation can reclaim input space incrementally.
fn merge_parts(paths: &[PathBuf], directory: &Path, part_rows: usize) -> Result<Vec<String>> {
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
    let mut outputs = Vec::new();
    while !heap.is_empty() {
        let name = format!("merge-{}", uuid::Uuid::new_v4());
        let mut out = BufWriter::new(File::create(directory.join(&name))?);
        for _ in 0..part_rows {
            let Some(Reverse((_, line, i))) = heap.pop() else {
                break;
            };
            out.write_all(line.as_bytes())?;
            if let Some((id, line)) = next(&mut readers[i])? {
                heap.push(Reverse((id, line, i)));
            }
        }
        out.flush()?;
        out.get_ref().sync_all()?;
        outputs.push(name);
    }
    Ok(outputs)
}
fn run(rows: &mut Vec<(String, Vec<String>)>, path: &Path) -> Result<()> {
    rows.sort();
    let mut writer = BufWriter::new(File::create(path)?);
    for row in rows.drain(..) {
        serde_json::to_writer(&mut writer, &row)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    writer.get_ref().sync_all()?;
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
    std::fs::create_dir_all(final_path.parent().context("missing dataset parent")?)?;
    // OS locking releases automatically on errors and killed processes.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(final_path.with_extension("prepare.lock"))?;
    lock.try_lock()
        .context("another process is preparing this Foursquare dataset")?;
    // Recheck after locking, in case another writer published while we waited.
    if final_path.exists() {
        let manifest = serde_json::from_reader(File::open(final_path.join("manifest.json"))?)?;
        return Ok((final_path, manifest));
    }
    let scratch = final_path.with_extension("preparing");
    std::fs::create_dir_all(&scratch)?;
    let checkpoint_path = scratch.join("checkpoint.json");
    let mut checkpoint: Checkpoint = if checkpoint_path.exists() {
        let checkpoint: Checkpoint = serde_json::from_reader(File::open(&checkpoint_path)?)?;
        if checkpoint.format != 1
            || checkpoint.manifest.input_fingerprint != fingerprint
            || checkpoint.manifest.region != region
            || checkpoint.manifest.source != "foursquare"
            || checkpoint.manifest.adapter_version != adapter_version(Source::Foursquare)
        {
            bail!("Foursquare preparation checkpoint does not match its input");
        }
        eprintln!(
            "Import: resuming Foursquare preparation ({:?}, {} of {} rows prepared)",
            checkpoint.stage, checkpoint.processed, checkpoint.total_rows
        );
        checkpoint
    } else {
        let mut reader = csv::Reader::from_path(input)?;
        let headers = reader.headers()?.clone();
        let mut manifest =
            prepare(Source::Foursquare, &row_csv(&headers, None)?, None, region)?.manifest;
        manifest.input_fingerprint = fingerprint.clone();
        let checkpoint = Checkpoint {
            format: 1,
            stage: Stage::Sorting,
            headers: headers.iter().map(str::to_owned).collect(),
            manifest,
            paths: Vec::new(),
            input_offset: reader.position().byte(),
            total_rows: 0,
            processed: 0,
            previous: None,
            output_bytes: 0,
            counts_ready: false,
            application: None,
        };
        checkpoint.save(&scratch)?;
        checkpoint
    };
    clean_runs(&scratch, &checkpoint)?;
    let result = prepare_checkpoint(input, &scratch, &mut checkpoint);
    if let Err(error) = result {
        eprintln!(
            "Import: saved preparation work in {}; rerun the same command to resume",
            scratch.display()
        );
        return Err(error);
    }
    for name in ["development.jsonl", "holdout.jsonl"] {
        std::fs::write(scratch.join(name), "")?;
    }
    std::fs::write(scratch.join("NOTICE.txt"), crate::foursquare::NOTICE)?;
    std::fs::write(scratch.join("LICENSE.txt"), crate::foursquare::LICENSE)?;
    std::fs::write(
        scratch.join("manifest.json"),
        serde_json::to_vec_pretty(&checkpoint.manifest)?,
    )?;
    // Keep the checkpoint until publication succeeds; failures remain resumable.
    for name in [
        "manifest.json",
        "NOTICE.txt",
        "LICENSE.txt",
        "development.jsonl",
        "holdout.jsonl",
    ] {
        File::open(scratch.join(name))?.sync_all()?;
    }
    File::open(&scratch)?.sync_all()?;
    std::fs::rename(&scratch, &final_path)?;
    File::open(final_path.parent().context("missing dataset parent")?)?.sync_all()?;
    std::fs::remove_file(final_path.join("checkpoint.json"))?;
    Ok((final_path, checkpoint.manifest))
}

fn prepare_checkpoint(input: &Path, scratch: &Path, checkpoint: &mut Checkpoint) -> Result<()> {
    sort_checkpoint(input, scratch, checkpoint, true)?;
    let headers = csv::StringRecord::from(checkpoint.headers.clone());
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .open(scratch.join("knowledge.json"))?;
    if output.metadata()?.len() < checkpoint.output_bytes {
        bail!("prepared output is shorter than its checkpoint");
    }
    // Discard partial/uncommitted writes, including a previous closing bracket.
    output.set_len(checkpoint.output_bytes)?;
    output.seek(std::io::SeekFrom::Start(checkpoint.output_bytes))?;
    let mut output = BufWriter::new(output);
    let mut progress =
        crate::import_progress::Progress::new("preparing place records", checkpoint.total_rows);
    progress.advance(checkpoint.processed);
    let mut prepared_rows = Vec::with_capacity(RUN);
    let fingerprint = checkpoint.manifest.input_fingerprint.clone();
    let region = checkpoint.manifest.region.clone();
    while let Some(name) = checkpoint.paths.first().cloned() {
        let path = scratch.join(&name);
        for line in BufReader::new(File::open(&path)?).lines() {
            let (id, fields): (String, Vec<String>) = serde_json::from_str(&line?)?;
            checkpoint.processed += 1;
            progress.advance(checkpoint.processed);
            if let Some((old_id, old_fields)) = &checkpoint.previous
                && old_id == &id
            {
                if old_fields != &fields {
                    bail!("conflicting Foursquare rows for place ID {id}");
                }
                checkpoint.manifest.skipped_places += 1;
                continue;
            }
            prepared_rows.push(csv::StringRecord::from(fields.clone()));
            if prepared_rows.len() == RUN {
                write_prepared(
                    &headers,
                    &mut prepared_rows,
                    &mut output,
                    &mut checkpoint.manifest,
                    &fingerprint,
                    &region,
                )?;
            }
            checkpoint.previous = Some((id, fields));
        }
        write_prepared(
            &headers,
            &mut prepared_rows,
            &mut output,
            &mut checkpoint.manifest,
            &fingerprint,
            &region,
        )?;
        output.flush()?;
        output.get_ref().sync_all()?;
        checkpoint.output_bytes = output.get_mut().stream_position()?;
        checkpoint.paths.remove(0);
        checkpoint.save(scratch)?;
        // Delete only after the output and its checkpoint are durable.
        std::fs::remove_file(path)?;
    }
    output.write_all(b"]")?;
    output.flush()?;
    output.get_ref().sync_all()?;
    progress.finish();
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/datasets/foursquare_file/tests.rs"]
mod tests;

fn sort_checkpoint(
    input: &Path,
    scratch: &Path,
    checkpoint: &mut Checkpoint,
    bundle: bool,
) -> Result<()> {
    let headers = csv::StringRecord::from(checkpoint.headers.clone());
    if checkpoint.stage == Stage::Sorting {
        let id_column = headers
            .iter()
            .position(|h| h == "fsq_place_id")
            .context("missing Foursquare CSV column fsq_place_id")?;
        let mut file = File::open(input)?;
        let bytes = usize::try_from(file.metadata()?.len())?;
        let offset = checkpoint.input_offset;
        file.seek(std::io::SeekFrom::Start(offset))?;
        let tracked = crate::import_progress::Reader::new(
            file,
            "sorting input CSV (bytes)",
            bytes - usize::try_from(offset)?,
        );
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .from_reader(tracked);
        let mut rows = Vec::with_capacity(RUN);
        let mut row = csv::StringRecord::new();
        let mut unsaved_runs = 0;
        while reader.read_record(&mut row)? {
            if row.len() != headers.len() {
                bail!(
                    "Foursquare CSV row has {} columns; expected {}",
                    row.len(),
                    headers.len()
                );
            }
            let id = row.get(id_column).unwrap_or("").trim().to_owned();
            if id.is_empty() {
                bail!("Foursquare place ID cannot be blank");
            }
            rows.push((id, row.iter().map(str::to_owned).collect()));
            checkpoint.total_rows += 1;
            if rows.len() == RUN {
                let name = format!("run-{}", uuid::Uuid::new_v4());
                run(&mut rows, &scratch.join(&name))?;
                checkpoint.paths.push(name);
                checkpoint.input_offset = offset + reader.position().byte();
                unsaved_runs += 1;
                // Bound checkpoint overhead while limiting replay to 64 runs.
                if unsaved_runs == SORT_CHECKPOINT_RUNS {
                    checkpoint.save(scratch)?;
                    unsaved_runs = 0;
                }
            }
        }
        if !rows.is_empty() {
            let name = format!("run-{}", uuid::Uuid::new_v4());
            run(&mut rows, &scratch.join(&name))?;
            checkpoint.paths.push(name);
        }
        checkpoint.input_offset = offset + reader.position().byte();
        checkpoint.stage = Stage::Merging;
        checkpoint.save(scratch)?;
    }
    if checkpoint.stage == Stage::Merging {
        let mut progress =
            crate::import_progress::Progress::new("merging sorted runs", checkpoint.paths.len());
        let mut processed = 0;
        while checkpoint.paths.len() > FAN_IN {
            let names = checkpoint.paths[..FAN_IN].to_vec();
            let paths: Vec<_> = names.iter().map(|name| scratch.join(name)).collect();
            let merged = merge_parts(&paths, scratch, usize::MAX)?;
            checkpoint.paths.drain(..FAN_IN);
            checkpoint.paths.extend(merged);
            checkpoint.save(scratch)?;
            for path in paths {
                std::fs::remove_file(path)?;
            }
            processed += FAN_IN - 1;
            progress.advance(processed);
        }
        let paths: Vec<_> = checkpoint
            .paths
            .iter()
            .map(|name| scratch.join(name))
            .collect();
        let parts = merge_parts(&paths, scratch, PART_ROWS)?;
        checkpoint.paths = parts;
        checkpoint.stage = Stage::Preparing;
        if bundle {
            let mut output = File::create(scratch.join("knowledge.json"))?;
            output.write_all(b"[")?;
            output.sync_all()?;
            checkpoint.output_bytes = 1;
        }
        checkpoint.save(scratch)?;
        for path in paths {
            std::fs::remove_file(path)?;
        }
        progress.finish();
    }
    Ok(())
}

#[path = "foursquare_stream.rs"]
mod streaming;
pub(super) use streaming::{import_file, resume_import};
