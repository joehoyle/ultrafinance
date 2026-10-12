//! Foursquare outlets share the source import transaction and are written in
//! bounded batches, including when merchant source inputs are unchanged.
use super::*;
use postgres::{binary_copy::BinaryCopyInWriter, types::Type};

pub(super) fn import(tx: &mut Transaction<'_>, records: &[SourceRecord]) -> Result<()> {
    if !records.iter().any(|r| r.source == "foursquare") {
        return Ok(());
    }
    tx.batch_execute(
        "CREATE TEMP TABLE IF NOT EXISTS fsq_location_stage(id TEXT,data TEXT) ON COMMIT DROP",
    )?;
    let total: usize = records
        .iter()
        .filter(|r| r.source == "foursquare")
        .map(|r| r.raw["places"].as_array().map_or(0, Vec::len))
        .sum();
    let mut progress =
        crate::import_progress::Progress::new("processing places for linked locations", total);
    let mut processed = 0;
    let mut written = 0;
    let mut pending = Vec::with_capacity(indexed_import::BATCH);
    for source in records {
        for record in crate::foursquare::locations(source)? {
            pending.push(record);
            if pending.len() == indexed_import::BATCH {
                write(tx, &pending)?;
                written += pending.len();
                pending.clear();
            }
        }
        if source.source == "foursquare" {
            processed += source.raw["places"].as_array().map_or(0, Vec::len);
        }
        progress.advance(processed);
    }
    write(tx, &pending)?;
    written += pending.len();
    progress.finish();
    crate::import_progress::locations(written, total - written);
    if crate::import_progress::verbose() {
        eprintln!(
            "Import: {written} addressed places submitted for location upsert; {} places without usable addresses",
            total - written
        );
    }
    Ok(())
}
fn write(tx: &mut Transaction<'_>, records: &[LocationRecord]) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let mut progress =
        crate::import_progress::Progress::new("writing linked locations", records.len());
    let sink = tx.copy_in("COPY fsq_location_stage FROM STDIN BINARY")?;
    let mut writer = BinaryCopyInWriter::new(sink, &[Type::TEXT, Type::TEXT]);
    for record in records {
        let id = format!("loc_{}", uuid::Uuid::new_v4().simple());
        writer.write(&[&id, &serde_json::to_string(record)?])?;
    }
    writer.finish()?;
    tx.batch_execute(r#"
        INSERT INTO source_provenance(source,attribution,license,url)
        SELECT DISTINCT data::jsonb->>'source',data::jsonb->>'attribution',data::jsonb->>'license',data::jsonb->>'url' FROM fsq_location_stage
        ON CONFLICT(source,attribution,license,url) DO NOTHING;
        INSERT INTO location_records(id,source,external_id,merchant_source,merchant_external_id,name,precision,address,city,region,postal_code,country,store_number,latitude,longitude,aliases_json,place_ids_json,transaction_pattern,manual_override,source_provenance_id)
        SELECT incoming.id,data->>'source',data->>'external_id',data#>>'{merchant,source}',data#>>'{merchant,external_id}',data#>>'{location,name}',data#>>'{location,precision}',data#>>'{location,address}',data#>>'{location,city}',data#>>'{location,region}',data#>>'{location,postal_code}',data#>>'{location,country}',data#>>'{location,store_number}',(data#>>'{location,latitude}')::double precision,(data#>>'{location,longitude}')::double precision,(data->'aliases')::text,(data->'place_ids')::text,data->>'transaction_pattern',(data->>'manual_override')::boolean,p.id
        FROM (SELECT id,data::jsonb AS data FROM fsq_location_stage) incoming JOIN source_provenance p ON (p.source,p.attribution,p.license,p.url)=(data->>'source',data->>'attribution',data->>'license',data->>'url')
        ON CONFLICT(source,external_id) DO UPDATE SET
            merchant_id=NULL,merchant_source=excluded.merchant_source,merchant_external_id=excluded.merchant_external_id,name=excluded.name,precision=excluded.precision,address=excluded.address,city=excluded.city,region=excluded.region,postal_code=excluded.postal_code,country=excluded.country,store_number=excluded.store_number,latitude=excluded.latitude,longitude=excluded.longitude,aliases_json=excluded.aliases_json,place_ids_json=excluded.place_ids_json,transaction_pattern=excluded.transaction_pattern,manual_override=excluded.manual_override,source_provenance_id=excluded.source_provenance_id
        WHERE NOT location_records.manual_override AND
            (location_records.merchant_id,location_records.merchant_source,location_records.merchant_external_id,location_records.name,location_records.precision,location_records.address,location_records.city,location_records.region,location_records.postal_code,location_records.country,location_records.store_number,location_records.latitude,location_records.longitude,location_records.aliases_json,location_records.place_ids_json,location_records.transaction_pattern,location_records.manual_override,location_records.source_provenance_id)
            IS DISTINCT FROM
            (excluded.merchant_id,excluded.merchant_source,excluded.merchant_external_id,excluded.name,excluded.precision,excluded.address,excluded.city,excluded.region,excluded.postal_code,excluded.country,excluded.store_number,excluded.latitude,excluded.longitude,excluded.aliases_json,excluded.place_ids_json,excluded.transaction_pattern,excluded.manual_override,excluded.source_provenance_id)
    "#)?;
    tx.batch_execute("TRUNCATE fsq_location_stage")?;
    progress.advance(records.len());
    progress.finish();
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/postgres_store/foursquare_locations/tests.rs"]
mod tests;
