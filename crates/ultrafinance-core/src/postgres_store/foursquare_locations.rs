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
        INSERT INTO location_records(id,source,external_id,merchant_source,merchant_external_id,name,precision,address,city,region,postal_code,country,store_number,latitude,longitude,aliases_json,place_ids_json,transaction_pattern,manual_override,attribution,license,url)
        SELECT id,data->>'source',data->>'external_id',data#>>'{merchant,source}',data#>>'{merchant,external_id}',data#>>'{location,name}',data#>>'{location,precision}',data#>>'{location,address}',data#>>'{location,city}',data#>>'{location,region}',data#>>'{location,postal_code}',data#>>'{location,country}',data#>>'{location,store_number}',(data#>>'{location,latitude}')::double precision,(data#>>'{location,longitude}')::double precision,(data->'aliases')::text,(data->'place_ids')::text,data->>'transaction_pattern',(data->>'manual_override')::boolean,data->>'attribution',data->>'license',data->>'url'
        FROM (SELECT id,data::jsonb AS data FROM fsq_location_stage) incoming
        ON CONFLICT(source,external_id) DO UPDATE SET
            merchant_id=NULL,merchant_source=excluded.merchant_source,merchant_external_id=excluded.merchant_external_id,name=excluded.name,precision=excluded.precision,address=excluded.address,city=excluded.city,region=excluded.region,postal_code=excluded.postal_code,country=excluded.country,store_number=excluded.store_number,latitude=excluded.latitude,longitude=excluded.longitude,aliases_json=excluded.aliases_json,place_ids_json=excluded.place_ids_json,transaction_pattern=excluded.transaction_pattern,manual_override=excluded.manual_override,attribution=excluded.attribution,license=excluded.license,url=excluded.url
        WHERE NOT location_records.manual_override
    "#)?;
    tx.batch_execute("TRUNCATE fsq_location_stage")?;
    progress.advance(records.len());
    progress.finish();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MerchantStore;
    use serde_json::json;

    fn record(id: usize) -> SourceRecord {
        let input = format!(
            "fsq_place_id,name,country,address,locality,region,postcode,latitude,longitude,fsq_category_ids,date_closed\n{id},Cafe,CA,{id} Main St,Toronto,ON,M5V,43.65,-79.38,\"[\"\"restaurant\"\"]\",\n"
        );
        crate::foursquare::prepare(&input, None, "ca")
            .unwrap()
            .0
            .remove(0)
    }

    #[test]
    fn country_refresh_replaces_stored_markets_without_evidence_rows() -> Result<()> {
        let store = MerchantStore::temporary()?;
        let mut input = record(0);
        store.reconcile_import(vec![input.clone()], false, 5000)?;
        let id = store
            .resolve_source("foursquare", &input.external_id)?
            .unwrap();
        assert_eq!(store.get(&id)?.unwrap().markets, ["CA"]);
        input.merchant.markets = vec!["US".into()];
        input.raw["places"][0]["country"] = json!("US");
        store.reconcile_import(vec![input], false, 5000)?;
        assert_eq!(store.get(&id)?.unwrap().markets, ["US"]);
        assert_eq!(
            store.locations(&id)?[0].location.country.as_deref(),
            Some("US")
        );
        assert_eq!(store.list(Some("CA"), 10, 0)?.total, 0);
        assert_eq!(store.list(Some("US"), 10, 0)?.total, 1);
        Ok(())
    }

    #[test]
    fn foursquare_existing_merchants_keep_distinct_outlets_and_refresh_ids() -> Result<()> {
        let store = MerchantStore::temporary()?;
        let mut merchant = record(0).merchant;
        merchant.id = "existing".into();
        store.put(&merchant)?;
        let before = store.fingerprint()?;
        store.reconcile_import(
            vec![record(0)],
            true,
            crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE,
        )?;
        assert_eq!(store.fingerprint()?, before);
        store.reconcile_import(
            vec![record(0)],
            false,
            crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE,
        )?;
        store.reconcile_import(
            vec![record(1)],
            false,
            crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE,
        )?;
        assert_eq!(store.stats()?.total, 1);
        let original = store.locations("existing")?;
        assert_eq!(original.len(), 2);
        let hydrated = store.list(None, 10, 0)?.merchants.remove(0);
        assert_eq!(hydrated.markets, ["CA"]);

        // A brand name alone must not resolve to a physical outlet.
        let request = serde_json::from_value(json!({"description":"Cafe"}))?;
        assert_eq!(
            crate::location::enrich(&request, &original).0,
            crate::location::LocationResult::default()
        );
        let zero = original
            .iter()
            .find(|r| r.external_id == "place:0")
            .unwrap();
        assert_eq!(zero.location.city.as_deref(), Some("Toronto"));
        assert_eq!(zero.location.latitude, Some(43.65));
        assert_eq!(zero.place_ids["foursquare"], "0");
        let mut refresh = record(0);
        refresh.raw["places"][0]["address"] = json!("Updated Street");
        let report = store.reconcile_import(
            vec![refresh.clone()],
            false,
            crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE,
        )?;
        assert_eq!(report.delta.unchanged, 1);
        let updated = store.locations("existing")?;
        let zero_updated = updated.iter().find(|r| r.external_id == "place:0").unwrap();
        assert_eq!(zero_updated.location.id, zero.location.id);
        assert_eq!(
            zero_updated.location.address.as_deref(),
            Some("Updated Street")
        );
        let mut manual = zero_updated.clone();
        manual.manual_override = true;
        manual.location.address = Some("Reviewed Street".into());
        store.import_locations(&[manual])?;
        store.reconcile_import(
            vec![refresh],
            false,
            crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE,
        )?;
        assert!(
            store
                .locations("existing")?
                .iter()
                .any(|r| r.location.address.as_deref() == Some("Reviewed Street"))
        );
        let mut target = merchant;
        target.id = "linked".into();
        target.name = "Other Merchant".into();
        store.put(&target)?;
        store.link("foursquare", "place:0", "linked")?;
        assert_eq!(store.locations("linked")?.len(), 1);
        assert_eq!(store.locations("existing")?.len(), 1);
        Ok(())
    }

    #[test]
    fn foursquare_reviewed_brand_and_direct_import_write_each_addressed_place() -> Result<()> {
        let store = MerchantStore::temporary()?;
        let mut brand = record(0);
        brand.external_id = "brand:cafe".into();
        let mut missing = record(2).raw["places"][0].clone();
        missing["address"] = json!("");
        brand.raw["places"] = json!([brand.raw["places"][0], record(1).raw["places"][0], missing]);
        store.import(std::slice::from_ref(&brand))?;
        let id = store.resolve_source("foursquare", "brand:cafe")?.unwrap();
        let outlets = store.locations(&id)?;
        assert_eq!(outlets.len(), 2);
        store.import(&[brand])?;
        let after = store.locations(&id)?;
        assert_eq!(
            outlets.iter().map(|r| &r.location.id).collect::<Vec<_>>(),
            after.iter().map(|r| &r.location.id).collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn foursquare_file_limits_and_late_errors_are_atomic_for_locations() -> Result<()> {
        let store = MerchantStore::temporary()?;
        let dir = std::env::temp_dir().join(format!("ultra-fsq-location-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir)?;
        let path = dir.join("knowledge.json");
        let mut records: Vec<_> = (0..=crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE)
            .map(record)
            .collect();
        let before = store.fingerprint()?;
        // First chunk has already written outlets when the final record fails.
        records[crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE].raw["places"][0]["fsq_place_id"] =
            json!(null);
        std::fs::write(&path, serde_json::to_vec(&records)?)?;
        assert!(
            store
                .reconcile_file(
                    path.clone(),
                    None,
                    false,
                    crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE
                )
                .is_err()
        );
        assert_eq!(store.fingerprint()?, before);
        records[crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE] =
            record(crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE);
        std::fs::write(&path, serde_json::to_vec(&records)?)?;
        store.reconcile_file(
            path.clone(),
            Some(1),
            false,
            crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE,
        )?;
        let id = store.resolve_source("foursquare", "place:0")?.unwrap();
        assert_eq!(store.locations(&id)?.len(), 1);
        store.reconcile_file(path, None, false, crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE)?;
        assert_eq!(store.stats()?.total, 1);
        assert_eq!(
            store.locations(&id)?.len(),
            crate::dedupe::DEFAULT_IMPORT_CHUNK_SIZE + 1
        );
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
