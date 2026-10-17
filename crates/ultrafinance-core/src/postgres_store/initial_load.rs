//! Atomic bootstrap for an empty catalog. Stage on disk, group rule identities,
//! retain correctness indexes, and build read-only secondary indexes once.
use super::*;
use postgres::{binary_copy::BinaryCopyInWriter, types::Type};

// Keep primary/unique keys and deterministic matching indexes active.
// Only secondary indexes unnecessary for a fresh Foursquare load are deferred.
// Keep merchant ownership lookups for canonical rebuilds. Only source-reference
// outlet lookups can be deferred: initial loads never refresh/relink old keys.
const DEFERRED: &[&str] = &[
    "merchants_name",
    "merchants_markets",
    "aliases_normalized",
    "merchant_search_tokens",
    "merchant_search_trigrams",
    "merchant_search_words_trigrams",
    "merchant_identity_name",
    "merchant_identity_host",
    "merchant_identity_trigrams",
    "merchant_identity_rule_host",
    "source_records_patterns",
    "location_records_source_merchant",
    "location_records_geography",
    "location_records_store_number",
];

pub(super) fn run<F>(
    client: &mut Client,
    total: usize,
    dry_run: bool,
    chunk_size: usize,
    produce: F,
) -> Result<crate::dedupe::ImportReport>
where
    F: FnOnce(&mut dyn FnMut(Vec<SourceRecord>) -> Result<()>) -> Result<()>,
{
    if chunk_size == 0 {
        bail!("import chunk size must be greater than zero");
    }
    let started = std::time::Instant::now();
    let mut tx = import_transaction(client)?;
    write_lock(&mut tx)?;
    // Lock before checking emptiness, including tables whose indexes are rebuilt.
    // Ordinary readers wait until the complete catalog and indexes commit.
    tx.batch_execute("LOCK TABLE merchants,manual_merchants,source_records,location_records,descriptor_resolutions,merchant_redirects,location_redirects,aliases,merchant_search,merchant_search_words,merchant_identity_keys IN ACCESS EXCLUSIVE MODE")?;
    let occupied: bool = tx.query_one("SELECT EXISTS(SELECT 1 FROM merchants) OR EXISTS(SELECT 1 FROM manual_merchants) OR EXISTS(SELECT 1 FROM source_records) OR EXISTS(SELECT 1 FROM location_records) OR EXISTS(SELECT 1 FROM merchant_redirects) OR EXISTS(SELECT 1 FROM descriptor_resolutions) OR EXISTS(SELECT 1 FROM location_redirects)", &[])?.get(0);
    if occupied {
        bail!("--initial-load requires an empty catalog; use the regular import for updates");
    }
    // Per-row foreign-key probes dominate otherwise sequential COPY writes.
    // Re-add validated constraints before commit: PostgreSQL checks the complete
    // relations in bulk, and any violation rolls back data and all schema edits.
    let foreign_keys = tx.query(
        "SELECT format('%I.%I',n.nspname,t.relname),quote_ident(c.conname),pg_get_constraintdef(c.oid) FROM pg_constraint c JOIN pg_class t ON t.oid=c.conrelid JOIN pg_namespace n ON n.oid=t.relnamespace WHERE c.contype='f' AND c.convalidated AND c.conrelid=ANY(ARRAY['source_records'::regclass,'location_records'::regclass,'aliases'::regclass,'merchant_search'::regclass,'merchant_identity_keys'::regclass]) ORDER BY t.relname,c.conname", &[])?;
    for key in &foreign_keys {
        tx.batch_execute(&format!(
            "ALTER TABLE {} DROP CONSTRAINT {}",
            key.get::<_, String>(0),
            key.get::<_, String>(1)
        ))?;
    }
    let indexes = tx.query(
        "SELECT format('%I.%I',n.nspname,c.relname),pg_get_indexdef(c.oid) FROM pg_index i JOIN pg_class c ON c.oid=i.indexrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=current_schema() AND c.relname=ANY($1) AND NOT i.indisunique ORDER BY c.relname",
        &[&DEFERRED],
    )?;
    for index in &indexes {
        tx.batch_execute(&format!("DROP INDEX {}", index.get::<_, String>(0)))?;
    }
    eprintln!(
        "Import: staging initial Foursquare load; {} secondary indexes deferred",
        indexes.len()
    );
    tx.batch_execute("CREATE TEMP TABLE initial_source_stage(rule_name TEXT,rule_host TEXT,source TEXT,external_id TEXT,data TEXT) ON COMMIT DROP")?;
    let mut staged = 0;
    let mut progress =
        crate::import_progress::Progress::new("staging initial Foursquare records", total);
    // One COPY stream for the complete dataset; the producer remains bounded.
    let sink = tx.copy_in("COPY initial_source_stage FROM STDIN BINARY")?;
    let mut writer = BinaryCopyInWriter::new(sink, &[Type::TEXT; 5]);
    produce(&mut |records| {
        for r in records {
            if r.source != "foursquare" {
                bail!("initial Foursquare load received another source");
            }
            validate(&r.merchant)?;
            if r.external_id.trim().is_empty() {
                bail!("source external ID cannot be blank");
            }
            let key = crate::dedupe::deterministic_key(&r.merchant);
            let name = key.as_ref().map(|k| k.0.as_str());
            let host = key.as_ref().map(|k| k.1.as_deref().unwrap_or(""));
            writer.write(&[
                &name,
                &host,
                &r.source,
                &r.external_id,
                &serde_json::to_string(&r)?,
            ])?;
            staged += 1;
        }
        progress.advance(staged);
        Ok(())
    })?;
    writer.finish()?;
    if staged != total {
        bail!("initial load record count changed: expected {total}, received {staged}");
    }
    progress.finish();
    eprintln!("Import: grouping staged identities and loading catalog");
    // PostgreSQL may spill this sort to disk. Source ordering within each rule
    // identity preserves canonical first-source precedence across chunk edges.
    tx.batch_execute("DECLARE initial_records NO SCROLL CURSOR FOR SELECT data FROM initial_source_stage ORDER BY rule_name,rule_host,source,external_id")?;
    let mut importer = indexed_import::Import::new_stream(&mut tx, dry_run, total, chunk_size)?;
    importer.staged()?;
    let report = importer.finish()?;
    tx.batch_execute("DROP TABLE initial_source_stage")?;
    let mut progress =
        crate::import_progress::Progress::new("building initial catalog indexes", indexes.len());
    for (n, index) in indexes.iter().enumerate() {
        tx.batch_execute(index.get::<_, &str>(1))?;
        progress.advance(n + 1);
    }
    progress.finish();
    tx.batch_execute("ANALYZE merchants,source_records,location_records,aliases,merchant_search,merchant_search_words,merchant_identity_keys")?;
    let mut progress = crate::import_progress::Progress::new(
        "validating initial catalog ownership",
        foreign_keys.len(),
    );
    for (n, key) in foreign_keys.iter().enumerate() {
        tx.batch_execute(&format!(
            "ALTER TABLE {} ADD CONSTRAINT {} {}",
            key.get::<_, String>(0),
            key.get::<_, String>(1),
            key.get::<_, String>(2)
        ))?;
        progress.advance(n + 1);
    }
    progress.finish();
    if dry_run {
        tx.rollback()?;
    } else {
        tx.commit()?;
    }
    eprintln!(
        "Import: initial load {} in {:.3}s; {staged} source records",
        if dry_run {
            "preview rolled back"
        } else {
            "committed"
        },
        started.elapsed().as_secs_f64()
    );
    Ok(report)
}

#[cfg(test)]
#[path = "../../tests/unit/postgres_store/initial_load/tests.rs"]
mod tests;
