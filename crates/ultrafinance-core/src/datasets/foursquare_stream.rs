//! Foursquare preparation feeds PostgreSQL directly; only sorted rows and small
//! durable cursors live on disk. A commit without a saved cursor is safely replayed.
use super::*;
use crate::{
    dedupe::ImportReport,
    store::{ImportDelta, MerchantStore},
};

#[derive(Default, Serialize, Deserialize)]
pub(super) struct ApplyCheckpoint {
    target: String,
    offset: u64,
    processed: usize,
    selected: usize,
    previous: Option<(String, Vec<String>)>,
    delta: ImportDelta,
    chunks: usize,
    candidates: usize,
    complete: bool,
}

fn prepared(
    headers: &csv::StringRecord,
    rows: &[csv::StringRecord],
    region: &str,
    fingerprint: &str,
) -> Result<(Vec<SourceRecord>, usize)> {
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(headers)?;
    for row in rows {
        writer.write_record(row)?;
    }
    let contents = String::from_utf8(writer.into_inner()?)?;
    let (mut records, skipped) = crate::foursquare::prepare(&contents, region)?;
    for record in &mut records {
        record.version = Some(format!("{fingerprint}:{region}:"));
    }
    Ok((records, skipped))
}

// Used for validation/counting and a single-transaction reconciliation preview.
// No prepared records are saved, and memory stays bounded by chunk_size.
fn scan<F>(
    paths: &[PathBuf],
    headers: &csv::StringRecord,
    manifest: &mut Manifest,
    chunk_size: usize,
    limit: Option<u32>,
    total_rows: usize,
    mut consume: F,
) -> Result<usize>
where
    F: FnMut(Vec<SourceRecord>) -> Result<()>,
{
    let mut progress =
        crate::import_progress::Progress::new("validating sorted place rows", total_rows);
    let mut processed = 0;
    let mut previous: Option<(String, Vec<String>)> = None;
    let mut pending = Vec::with_capacity(chunk_size);
    let mut selected = 0;
    let fingerprint = manifest.input_fingerprint.clone();
    let region = manifest.region.clone();
    let mut flush = |pending: &mut Vec<csv::StringRecord>, manifest: &mut Manifest| -> Result<()> {
        let (mut records, skipped) = prepared(headers, pending, &region, &fingerprint)?;
        pending.clear();
        manifest.skipped_places += skipped;
        manifest.merchant_records += records.len();
        manifest.retained_places += records.len();
        let remaining = limit.map_or(usize::MAX, |n| (n as usize).saturating_sub(selected));
        records.truncate(remaining);
        selected += records.len();
        if !records.is_empty() {
            consume(records)?;
        }
        Ok(())
    };
    for path in paths {
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
            pending.push(csv::StringRecord::from(fields.clone()));
            previous = Some((id, fields));
            if pending.len() == chunk_size {
                flush(&mut pending, manifest)?;
            }
        }
    }
    if !pending.is_empty() {
        flush(&mut pending, manifest)?;
    }
    if processed != total_rows {
        bail!("sorted input row count does not match its checkpoint");
    }
    progress.finish();
    Ok(selected)
}

fn empty_manifest(
    headers: &csv::StringRecord,
    region: &str,
    fingerprint: &str,
) -> Result<Manifest> {
    let mut manifest =
        prepare(Source::Foursquare, &row_csv(headers, None)?, None, region)?.manifest;
    manifest.input_fingerprint = fingerprint.to_owned();
    Ok(manifest)
}

fn validate_resume(checkpoint: &Checkpoint, region: &str) -> Result<()> {
    if checkpoint.format != 1
        || checkpoint.stage != Stage::Preparing
        || !checkpoint.counts_ready
        || checkpoint.application.is_none()
        || checkpoint.manifest.source != "foursquare"
        || checkpoint.manifest.region != region
        || checkpoint.manifest.adapter_version != adapter_version(Source::Foursquare)
        || checkpoint.manifest.input_fingerprint.is_empty()
    {
        bail!(
            "checkpoint-only resume requires a validated Foursquare import checkpoint matching this region and adapter version"
        );
    }
    Ok(())
}

pub(in crate::datasets) fn import_file(
    input: &Path,
    directory: &Path,
    region: &str,
    options: FoursquareImportOptions,
) -> Result<Value> {
    import_from(Some(input), directory, region, options, None)
}

pub(in crate::datasets) fn resume_import(
    checkpoint: &Path,
    region: &str,
    options: FoursquareImportOptions,
) -> Result<Value> {
    import_from(None, Path::new(""), region, options, Some(checkpoint))
}

fn import_from(
    input: Option<&Path>,
    directory: &Path,
    region: &str,
    options: FoursquareImportOptions,
    resume: Option<&Path>,
) -> Result<Value> {
    let chunk_size = options.dedupe.chunk_size;
    if chunk_size == 0 {
        bail!("import chunk size must be greater than zero");
    }
    let preview = options.dry_run || options.dedupe.dry_run;
    if resume.is_some() && preview {
        bail!(
            "checkpoint-only resume does not support dry runs; consumed input files are no longer available"
        );
    }
    let resumed_directory = resume
        .map(|path| -> Result<PathBuf> {
            let path = if path
                .file_name()
                .is_some_and(|name| name == "checkpoint.json")
            {
                path.parent()
                    .context("checkpoint has no parent directory")?
            } else {
                path
            };
            let path = path
                .canonicalize()
                .context("resume checkpoint directory not found")?;
            let checkpoint: Checkpoint =
                serde_json::from_reader(File::open(path.join("checkpoint.json"))?)?;
            validate_resume(&checkpoint, region)?;
            Ok(path)
        })
        .transpose()?;
    let store = if options.dry_run {
        None
    } else {
        Some(MerchantStore::configured(options.database_url.as_deref())?)
    };
    let target = if let Some(store) = &store {
        let config: postgres::Config = options
            .database_url
            .as_deref()
            .unwrap_or(crate::store::LOCAL_DATABASE_URL)
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid PostgreSQL configuration"))?;
        // Passwords/user names are excluded, allowing renewed credentials on resume.
        crate::eval::fingerprint(
            format!(
                "{:?}:{:?}:{}",
                config.get_hosts(),
                config.get_ports(),
                store.import_database_identity()?
            )
            .as_bytes(),
        )
    } else {
        "preview".to_owned()
    };
    let (fingerprint, scratch) = if let Some(scratch) = resumed_directory {
        let checkpoint: Checkpoint =
            serde_json::from_reader(File::open(scratch.join("checkpoint.json"))?)?;
        (checkpoint.manifest.input_fingerprint, scratch)
    } else {
        let input = input.context("missing Foursquare input CSV")?;
        let fingerprint = fingerprint_file(input)?;
        let base = bundle_path(
            directory,
            "foursquare",
            adapter_version(Source::Foursquare),
            region,
            &fingerprint,
            None,
        );
        let scratch = base.with_extension(if preview { "preview" } else { "streaming" });
        std::fs::create_dir_all(&scratch)?;
        (fingerprint, scratch)
    };
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(scratch.with_extension(if preview {
            "preview.lock"
        } else {
            "streaming.lock"
        }))?;
    lock.try_lock()
        .context("another process is importing this Foursquare dataset")?;
    let checkpoint_path = scratch.join("checkpoint.json");
    let existing: Option<Checkpoint> = if checkpoint_path.exists() {
        Some(serde_json::from_reader(File::open(&checkpoint_path)?)?)
    } else {
        None
    };
    if resume.is_some() {
        validate_resume(
            existing.as_ref().context("resume checkpoint missing")?,
            region,
        )?;
    }
    if let Some(checkpoint) = &existing
        && let Some(apply) = &checkpoint.application
        && apply.target != target
    {
        bail!(
            "Foursquare checkpoint belongs to a different database; use a different --output directory"
        );
    }
    let mut checkpoint = if let Some(checkpoint) =
        existing.filter(|c| resume.is_some() || c.application.as_ref().is_none_or(|a| !a.complete))
    {
        if checkpoint.format != 1
            || checkpoint.manifest.input_fingerprint != fingerprint
            || checkpoint.manifest.region != region
            || checkpoint.manifest.adapter_version != adapter_version(Source::Foursquare)
        {
            bail!("Foursquare checkpoint does not match its input");
        }
        checkpoint
    } else {
        let mut reader = csv::Reader::from_path(input.context("missing Foursquare input CSV")?)?;
        let headers = reader.headers()?.clone();
        let checkpoint = Checkpoint {
            format: 1,
            stage: Stage::Sorting,
            headers: headers.iter().map(str::to_owned).collect(),
            manifest: empty_manifest(&headers, region, &fingerprint)?,
            paths: vec![],
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
    if resume.is_none() {
        sort_checkpoint(
            input.context("missing Foursquare input CSV")?,
            &scratch,
            &mut checkpoint,
            false,
        )?;
    }
    for (name, contents) in [
        ("NOTICE.txt", crate::foursquare::NOTICE),
        ("LICENSE.txt", crate::foursquare::LICENSE),
    ] {
        std::fs::write(scratch.join(name), contents)?;
    }
    let headers = csv::StringRecord::from(checkpoint.headers.clone());
    if !checkpoint.counts_ready {
        eprintln!("Import: validating and counting sorted places (no JSON bundle)");
        let paths: Vec<_> = checkpoint
            .paths
            .iter()
            .map(|name| scratch.join(name))
            .collect();
        let mut manifest = empty_manifest(&headers, region, &fingerprint)?;
        scan(
            &paths,
            &headers,
            &mut manifest,
            chunk_size,
            None,
            checkpoint.total_rows,
            |_| Ok(()),
        )?;
        checkpoint.manifest = manifest;
        checkpoint.counts_ready = true;
        checkpoint.save(&scratch)?;
    }
    let available = checkpoint.manifest.merchant_records;
    let selected_total = options
        .limit
        .map_or(available, |n| available.min(n as usize));
    eprintln!("Import: {available} available source records; {selected_total} selected");
    if preview {
        let paths: Vec<_> = checkpoint
            .paths
            .iter()
            .map(|name| scratch.join(name))
            .collect();
        let mut report = json!({"bundle":null,"checkpoint":scratch,"manifest":checkpoint.manifest,"dry_run":true,
            "selection":{"available_records":available,"selected_records":selected_total,"limit":options.limit,"chunk_size":chunk_size}});
        if let Some(store) = store {
            let mut manifest = empty_manifest(&headers, region, &fingerprint)?;
            let total_rows = checkpoint.total_rows;
            let result =
                store.reconcile_generated(selected_total, true, chunk_size, move |consume| {
                    scan(
                        &paths,
                        &headers,
                        &mut manifest,
                        chunk_size,
                        options.limit,
                        total_rows,
                        consume,
                    )?;
                    Ok(())
                })?;
            report["delta"] = json!(result.delta);
            report["dedupe"] = json!(result.dedupe);
            eprintln!("Import: identity preview complete; no database changes");
        }
        return Ok(report);
    }
    let store = store.context("missing import database")?;
    let resumed = checkpoint.application.as_ref().map_or(0, |a| a.selected);
    if resumed > selected_total {
        bail!(
            "--limit is below the saved import position; increase it or use a different --output directory"
        );
    }
    if checkpoint.application.is_none() {
        checkpoint.application = Some(ApplyCheckpoint {
            target,
            ..Default::default()
        });
        checkpoint.save(&scratch)?;
    }
    eprintln!(
        "Import: resuming at {resumed} committed source records; committing at most {chunk_size} records per chunk"
    );
    let start_delta = {
        let apply = checkpoint
            .application
            .as_ref()
            .context("missing application checkpoint")?;
        (
            apply.delta.added,
            apply.delta.updated,
            apply.delta.unchanged,
        )
    };
    let mut progress =
        crate::import_progress::Progress::new("importing sorted place rows", checkpoint.total_rows);
    loop {
        let apply = checkpoint
            .application
            .as_mut()
            .context("missing application checkpoint")?;
        progress.advance(apply.processed);
        if apply.selected == selected_total {
            break;
        }
        if checkpoint.paths.is_empty() {
            bail!("sorted input ended before the selected record count");
        }
        let path = scratch.join(&checkpoint.paths[0]);
        let mut file = File::open(&path)?;
        file.seek(std::io::SeekFrom::Start(apply.offset))?;
        let mut reader = BufReader::new(file);
        let mut rows = Vec::with_capacity(chunk_size);
        let bound = chunk_size.min(selected_total - apply.selected);
        let mut eof = false;
        while rows.len() < bound {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                eof = true;
                break;
            }
            let (id, fields): (String, Vec<String>) = serde_json::from_str(&line)?;
            apply.processed += 1;
            if let Some((old_id, old_fields)) = &apply.previous
                && old_id == &id
            {
                if old_fields != &fields {
                    bail!("conflicting Foursquare rows for place ID {id}");
                }
                continue;
            }
            rows.push(csv::StringRecord::from(fields.clone()));
            apply.previous = Some((id, fields));
        }
        eof |= reader.fill_buf()?.is_empty();
        apply.offset = reader.stream_position()?;
        let (records, _) = prepared(&headers, &rows, region, &fingerprint)?;
        let count = records.len();
        if count > 0 {
            let result: ImportReport =
                store.reconcile_generated(count, false, chunk_size, move |consume| {
                    consume(records)
                })?;
            apply.delta.added += result.delta.added;
            apply.delta.updated += result.delta.updated;
            apply.delta.unchanged += result.delta.unchanged;
            apply.candidates += result.dedupe.candidates;
            apply.selected += count;
            apply.chunks += 1;
        }
        if eof {
            checkpoint.paths.remove(0);
            apply.offset = 0;
        }
        // A failed cursor save leaves the old chunk available for delta replay.
        checkpoint.save(&scratch).context(
            "database chunk may have committed; rerun the same command to safely replay it",
        )?;
        if eof {
            std::fs::remove_file(path)?;
        }
    }
    // Finish consuming duplicate/skipped trailing rows only when all eligible
    // records were selected; otherwise preserve them for a larger future limit.
    if selected_total == available {
        let apply = checkpoint
            .application
            .as_mut()
            .context("missing application checkpoint")?;
        apply.complete = true;
        apply.processed = checkpoint.total_rows;
        checkpoint.paths.clear();
        checkpoint.save(&scratch)?;
        clean_runs(&scratch, &checkpoint)?;
        progress.finish();
    }
    let apply = checkpoint
        .application
        .as_ref()
        .context("missing application checkpoint")?;
    eprintln!(
        "Import: committed {} source records in {} chunks; {} added, {} updated, {} unchanged",
        apply.selected, apply.chunks, apply.delta.added, apply.delta.updated, apply.delta.unchanged
    );
    Ok(
        json!({"bundle":null,"checkpoint":scratch,"manifest":checkpoint.manifest,"dry_run":false,
        "selection":{"available_records":available,"selected_records":apply.selected,"limit":options.limit,"chunk_size":chunk_size},
        "resume":{"previously_committed":resumed,"committed_this_run":apply.selected-resumed,"complete":apply.complete},
        "delta":{"added":apply.delta.added-start_delta.0,"updated":apply.delta.updated-start_delta.1,"unchanged":apply.delta.unchanged-start_delta.2},"dedupe":{"dry_run":false,"model":"deterministic","candidates":apply.candidates,"errors":0,"details_truncated":true},"committed_chunks":apply.chunks}),
    )
}

#[cfg(test)]
#[path = "../../tests/unit/datasets/foursquare_stream/tests.rs"]
mod tests;
