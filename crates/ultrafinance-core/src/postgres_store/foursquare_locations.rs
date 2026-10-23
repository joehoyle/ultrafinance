//! Foursquare outlets share the source import transaction and are written in
//! bounded batches, including when merchant source inputs are unchanged.
use super::*;
use postgres::{binary_copy::BinaryCopyInWriter, types::Type};

pub(super) fn import_owned(
    tx: &mut Transaction<'_>,
    records: &[SourceRecord],
    identities: &[i64],
    initial: bool,
) -> Result<()> {
    if records.len() != identities.len() {
        bail!("location owner count does not match source inputs");
    }
    let owners = records
        .iter()
        .zip(identities)
        .map(|(r, id)| ((r.source.as_str(), r.external_id.as_str()), *id))
        .collect();
    import_with_owners(tx, records, Some(&owners), initial)
}

type Owners<'a> = HashMap<(&'a str, &'a str), i64>;
fn import_with_owners(
    tx: &mut Transaction<'_>,
    records: &[SourceRecord],
    owners: Option<&Owners<'_>>,
    initial: bool,
) -> Result<()> {
    if !records.iter().any(|r| r.source == "foursquare") {
        return Ok(());
    }
    stage(tx)?;
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
                write(tx, &pending, owners, initial)?;
                written += pending.len();
                pending.clear();
            }
        }
        if source.source == "foursquare" {
            processed += source.raw["places"].as_array().map_or(0, Vec::len);
        }
        progress.advance(processed);
    }
    write(tx, &pending, owners, initial)?;
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
/// Write an already compared delta; unchanged outlets never enter COPY/upsert.
pub(super) fn import_changed(
    tx: &mut Transaction<'_>,
    records: &[LocationRecord],
    identities: &HashMap<String, i64>,
) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    stage(tx)?;
    let owners: Owners<'_> = records
        .iter()
        .map(|r| {
            let crate::location::MerchantReference::Source {
                source,
                external_id,
            } = &r.merchant
            else {
                bail!("Foursquare locations require source identities");
            };
            let owner = identities
                .get(external_id)
                .context("changed outlet has no source owner")?;
            Ok(((source.as_str(), external_id.as_str()), *owner))
        })
        .collect::<Result<_>>()?;
    for chunk in records.chunks(indexed_import::BATCH) {
        write(tx, chunk, Some(&owners), false)?;
    }
    Ok(())
}

fn stage(tx: &mut Transaction<'_>) -> Result<()> {
    tx.batch_execute(
        "CREATE TEMP TABLE IF NOT EXISTS fsq_location_stage(id TEXT,source TEXT,external_id TEXT,merchant_source TEXT,merchant_external_id TEXT,name TEXT,precision TEXT,address TEXT,city TEXT,region TEXT,postal_code TEXT,country TEXT,store_number TEXT,latitude DOUBLE PRECISION,longitude DOUBLE PRECISION,aliases TEXT[],transaction_pattern TEXT,manual_override BOOLEAN,attribution TEXT,license TEXT,url TEXT,merchant_id BIGINT) ON COMMIT DROP",
    )?;
    Ok(())
}

fn write(
    tx: &mut Transaction<'_>,
    records: &[LocationRecord],
    owners: Option<&Owners<'_>>,
    initial: bool,
) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let mut progress =
        crate::import_progress::Progress::new("writing linked locations", records.len());
    let sink = tx.copy_in("COPY fsq_location_stage FROM STDIN BINARY")?;
    let mut types = vec![Type::TEXT; 22];
    types[13] = Type::FLOAT8;
    types[14] = Type::FLOAT8;
    types[15] = Type::TEXT_ARRAY;
    types[17] = Type::BOOL;
    types[21] = Type::INT8;
    let mut writer = BinaryCopyInWriter::new(sink, &types);
    for record in records {
        let id = format!("loc_{}", uuid::Uuid::new_v4().simple());
        let crate::location::MerchantReference::Source {
            source,
            external_id,
        } = &record.merchant
        else {
            bail!("Foursquare locations require source merchant references");
        };
        let merchant_id = owners
            .map(|owners| {
                owners
                    .get(&(source.as_str(), external_id.as_str()))
                    .copied()
                    .context("initial location has no reconciled source owner")
            })
            .transpose()?;
        let location = &record.location;
        let precision = serde_json::to_value(location.precision)?;
        let precision = precision.as_str();
        writer.write(&[
            &id,
            &record.source,
            &record.external_id,
            source,
            external_id,
            &location.name,
            &precision,
            &location.address,
            &location.city,
            &location.region,
            &location.postal_code,
            &location.country,
            &location.store_number,
            &location.latitude,
            &location.longitude,
            &record.aliases,
            &record.transaction_pattern,
            &record.manual_override,
            &record.attribution,
            &record.license,
            &record.url,
            &merchant_id,
        ])?;
    }
    writer.finish()?;
    // Transport typed fields directly. Serializing each outlet as JSON and
    // parsing it again for provenance and every SQL field adds avoidable CPU.
    tx.batch_execute(
        r#"
        INSERT INTO source_provenance(source,attribution,license,url)
        SELECT DISTINCT source,attribution,license,url FROM fsq_location_stage
        ON CONFLICT(source,attribution,license,url) DO NOTHING;
    "#,
    )?;
    if initial {
        // No existing outlets or overrides can exist in this mode. Keep unique
        // constraints active, but avoid speculative upserts and row-trigger
        // lookups of ownership we already know from the source import.
        tx.batch_execute(r#"
        INSERT INTO location_records(id,source,external_id,merchant_id,merchant_source,merchant_external_id,name,precision,address,city,region,postal_code,country,store_number,latitude,longitude,aliases,transaction_pattern,manual_override,source_provenance_id)
        SELECT s.id,s.source,s.external_id,s.merchant_id,NULL,NULL,s.name,s.precision,s.address,s.city,s.region,s.postal_code,s.country,s.store_number,s.latitude,s.longitude,s.aliases,s.transaction_pattern,s.manual_override,p.id
        FROM fsq_location_stage s JOIN source_provenance p ON (p.source,p.attribution,p.license,p.url)=(s.source,s.attribution,s.license,s.url)
        "#)?;
    } else {
        tx.batch_execute(r#"
        INSERT INTO location_records(id,source,external_id,merchant_id,merchant_source,merchant_external_id,name,precision,address,city,region,postal_code,country,store_number,latitude,longitude,aliases,transaction_pattern,manual_override,source_provenance_id)
        SELECT s.id,s.source,s.external_id,s.merchant_id,NULL,NULL,s.name,s.precision,s.address,s.city,s.region,s.postal_code,s.country,s.store_number,s.latitude,s.longitude,s.aliases,s.transaction_pattern,s.manual_override,p.id
        FROM fsq_location_stage s JOIN source_provenance p ON (p.source,p.attribution,p.license,p.url)=(s.source,s.attribution,s.license,s.url)
        ON CONFLICT(source,external_id) DO UPDATE SET
            merchant_id=excluded.merchant_id,merchant_source=excluded.merchant_source,merchant_external_id=excluded.merchant_external_id,name=excluded.name,precision=excluded.precision,address=excluded.address,city=excluded.city,region=excluded.region,postal_code=excluded.postal_code,country=excluded.country,store_number=excluded.store_number,latitude=excluded.latitude,longitude=excluded.longitude,aliases=excluded.aliases,transaction_pattern=excluded.transaction_pattern,manual_override=excluded.manual_override,source_provenance_id=excluded.source_provenance_id
        WHERE NOT location_records.manual_override AND
            (location_records.merchant_id,location_records.merchant_source,location_records.merchant_external_id,location_records.name,location_records.precision,location_records.address,location_records.city,location_records.region,location_records.postal_code,location_records.country,location_records.store_number,location_records.latitude,location_records.longitude,location_records.aliases,location_records.transaction_pattern,location_records.manual_override,location_records.source_provenance_id)
            IS DISTINCT FROM
            (excluded.merchant_id,excluded.merchant_source,excluded.merchant_external_id,excluded.name,excluded.precision,excluded.address,excluded.city,excluded.region,excluded.postal_code,excluded.country,excluded.store_number,excluded.latitude,excluded.longitude,excluded.aliases,excluded.transaction_pattern,excluded.manual_override,excluded.source_provenance_id)
    "#)?;
    }
    tx.batch_execute("TRUNCATE fsq_location_stage")?;
    progress.advance(records.len());
    progress.finish();
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/postgres_store/foursquare_locations/tests.rs"]
mod tests;
