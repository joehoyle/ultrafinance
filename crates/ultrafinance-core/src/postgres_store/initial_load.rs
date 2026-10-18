//! Atomic bootstrap for an empty catalog. Stage on disk, group rule identities,
//! write final rows once, then rebuild and validate constraints and indexes.
use super::*;
use postgres::{binary_copy::BinaryCopyInWriter, types::Type};

// All catalog lookups are unnecessary for a grouped empty-catalog load.
// Build secondary indexes once after final merchant/source/location writes.
const DEFERRED: &[&str] = &[
    "merchants_name",
    "merchants_markets",
    "aliases_normalized",
    "merchant_search_tokens",
    "merchant_search_words_trigrams",
    "merchant_identity_rule",
    "merchant_identity_host",
    "merchant_identity_trigrams",
    "merchant_identity_rule_host",
    "source_records_merchant",
    "foursquare_unlocated_merchant",
    "source_records_patterns",
    "location_records_merchant",
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
    tx.batch_execute("LOCK TABLE foursquare_unlocated,merchants,manual_merchants,source_records,location_records,descriptor_resolutions,merchant_redirects,location_redirects,aliases,merchant_search,merchant_search_words,merchant_identity_keys IN ACCESS EXCLUSIVE MODE")?;
    let occupied: bool = tx.query_one("SELECT EXISTS(SELECT 1 FROM foursquare_unlocated) OR EXISTS(SELECT 1 FROM merchants) OR EXISTS(SELECT 1 FROM manual_merchants) OR EXISTS(SELECT 1 FROM source_records) OR EXISTS(SELECT 1 FROM location_records) OR EXISTS(SELECT 1 FROM merchant_redirects) OR EXISTS(SELECT 1 FROM descriptor_resolutions) OR EXISTS(SELECT 1 FROM location_redirects)", &[])?.get(0);
    if occupied {
        bail!("--initial-load requires an empty catalog; use the regular import for updates");
    }
    // Per-row foreign-key probes dominate otherwise sequential COPY writes.
    // Re-add validated constraints before commit: PostgreSQL checks the complete
    // relations in bulk, and any violation rolls back data and all schema edits.
    let foreign_keys = tx.query(
        "SELECT format('%I.%I',n.nspname,t.relname),quote_ident(c.conname),pg_get_constraintdef(c.oid) FROM pg_constraint c JOIN pg_class t ON t.oid=c.conrelid JOIN pg_namespace n ON n.oid=t.relnamespace WHERE c.contype='f' AND c.convalidated AND (c.conrelid=ANY(ARRAY['foursquare_unlocated'::regclass,'source_records'::regclass,'location_records'::regclass,'aliases'::regclass,'merchant_search'::regclass,'merchant_identity_keys'::regclass]) OR c.confrelid=ANY(ARRAY['merchants'::regclass,'source_records'::regclass,'location_records'::regclass,'aliases'::regclass,'merchant_search'::regclass,'merchant_identity_keys'::regclass])) ORDER BY t.relname,c.conname", &[])?;
    for key in &foreign_keys {
        tx.batch_execute(&format!(
            "ALTER TABLE {} DROP CONSTRAINT {}",
            key.get::<_, String>(0),
            key.get::<_, String>(1)
        ))?;
    }
    // The grouped loader never probes these keys while loading. Rebuild their
    // uniqueness constraints before restoring FKs; duplicate inputs abort then.
    let unique_keys = tx.query("SELECT format('%I.%I',n.nspname,t.relname),quote_ident(c.conname),pg_get_constraintdef(c.oid) FROM pg_constraint c JOIN pg_class t ON t.oid=c.conrelid JOIN pg_namespace n ON n.oid=t.relnamespace WHERE n.nspname=current_schema() AND c.conname=ANY($1) AND c.contype IN ('p','u') ORDER BY t.relname,c.conname", &[&vec!["merchants_pkey","merchants_key_unique","source_records_pkey","location_records_pkey","location_records_source_external_id_key","aliases_pkey","merchant_search_pkey","merchant_identity_keys_pkey"]])?;
    for key in &unique_keys {
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
    // Only this ownership resolver is deferred. Source relinking and custom
    // triggers remain active, and a set-based check validates owners before commit.
    let location_trigger_mode: String = tx.query_one("SELECT tgenabled::text FROM pg_trigger WHERE tgrelid='location_records'::regclass AND tgname='location_merchant'", &[])?.get(0);
    tx.batch_execute("ALTER TABLE location_records DISABLE TRIGGER location_merchant")?;
    // Build the vocabulary from final search rows once, rather than probing
    // its unique index for common words on every import batch. Preserve each
    // trigger's original enabled mode; rollback restores it automatically.
    let word_triggers = tx.query("SELECT quote_ident(tgname),tgenabled::text FROM pg_trigger WHERE tgrelid='merchant_search'::regclass AND tgname IN ('merchant_search_words_insert','merchant_search_words_update')", &[])?;
    for trigger in &word_triggers {
        tx.batch_execute(&format!(
            "ALTER TABLE merchant_search DISABLE TRIGGER {}",
            trigger.get::<_, String>(0)
        ))?;
    }
    eprintln!(
        "Import: staging initial Foursquare load; {} secondary indexes deferred",
        indexes.len()
    );
    tx.batch_execute("CREATE TEMP TABLE initial_identity_owners(external_id TEXT,merchant_id BIGINT) ON COMMIT DROP; CREATE TEMP TABLE initial_source_stage(rule_name TEXT,rule_host TEXT,source TEXT,external_id TEXT,data TEXT) ON COMMIT DROP")?;
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
    // `total` is a progress upper bound; filtering happens during staging.
    progress.finish();
    let duplicate: bool = tx.query_one("SELECT EXISTS(SELECT 1 FROM initial_source_stage GROUP BY source,external_id HAVING count(*)>1)", &[])?.get(0);
    if duplicate {
        bail!("initial load requires unique provider IDs");
    }
    eprintln!("Import: grouping staged identities and loading catalog");
    // PostgreSQL may spill this sort to disk. Source ordering within each rule
    // identity preserves canonical first-source precedence across chunk edges.
    tx.batch_execute("DECLARE initial_records NO SCROLL CURSOR FOR SELECT rule_name,rule_host,data FROM initial_source_stage ORDER BY rule_name,rule_host,source,external_id")?;
    let report = grouped::load(&mut tx, staged, chunk_size, dry_run)?;
    tx.batch_execute("DROP TABLE initial_source_stage")?;
    eprintln!("Import: building search vocabulary from final merchant names");
    tx.batch_execute("INSERT INTO merchant_search_words(word) SELECT DISTINCT word FROM merchant_search CROSS JOIN LATERAL unnest(tsvector_to_array(tokens)) words(word) WHERE char_length(word)>=3 ON CONFLICT(word) DO NOTHING")?;
    for trigger in &word_triggers {
        restore_trigger(
            &mut tx,
            "merchant_search",
            &trigger.get::<_, String>(0),
            trigger.get::<_, &str>(1),
        )?;
    }
    eprintln!("Import: building and validating initial unique keys");
    for key in &unique_keys {
        tx.batch_execute(&format!(
            "ALTER TABLE {} ADD CONSTRAINT {} {}",
            key.get::<_, String>(0),
            key.get::<_, String>(1),
            key.get::<_, String>(2)
        ))?;
    }
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
    let invalid_identity: bool = tx.query_one("SELECT EXISTS(SELECT external_id,merchant_id FROM initial_identity_owners EXCEPT (SELECT external_id,merchant_id FROM location_records WHERE source='foursquare' UNION ALL SELECT external_id,merchant_id FROM foursquare_unlocated))", &[])?.get(0);
    if invalid_identity {
        bail!("Foursquare import identity ownership does not match its merchant");
    }
    eprintln!("Import: validating source-linked location ownership");
    let invalid_owner: bool = tx.query_one("SELECT EXISTS(SELECT 1 FROM location_records l LEFT JOIN source_records s ON (s.source,s.external_id)=(l.merchant_source,l.merchant_external_id) WHERE l.merchant_source IS NOT NULL AND merchant_key(s.merchant_id) IS DISTINCT FROM l.merchant_id)", &[])?.get(0);
    if invalid_owner {
        bail!("initial location ownership does not match its source merchant");
    }
    restore_trigger(
        &mut tx,
        "location_records",
        "location_merchant",
        &location_trigger_mode,
    )?;
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

// Names come from fixed application identifiers or quote_ident in pg_catalog.
fn restore_trigger(tx: &mut Transaction<'_>, table: &str, trigger: &str, mode: &str) -> Result<()> {
    let action = match mode {
        "O" => "ENABLE",
        "A" => "ENABLE ALWAYS",
        "R" => "ENABLE REPLICA",
        "D" => "DISABLE",
        _ => bail!("unsupported trigger mode"),
    };
    tx.batch_execute(&format!("ALTER TABLE {table} {action} TRIGGER {trigger}"))?;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/postgres_store/initial_load/tests.rs"]
mod tests;

#[path = "initial_load/grouped.rs"]
mod grouped;
