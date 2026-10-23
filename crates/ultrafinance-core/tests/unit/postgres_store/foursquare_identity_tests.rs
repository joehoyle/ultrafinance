use super::*;
use crate::store::MerchantStore;

fn records() -> Result<Vec<SourceRecord>> {
    Ok(crate::foursquare::prepare("fsq_place_id,name,country,address,website,fsq_category_ids,date_closed\n1,Cafe,CA,1 Main St,https://cafe.test,[\"shop\"],\n2,Cafe,CA,,https://cafe.test,[\"shop\"],\n", "ca")?.0)
}

#[test]
fn fingerprint_replay_skips_reconciliation_and_ignores_snapshot_version() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let mut input = records()?.remove(0);
    store.reconcile_import(vec![input.clone()], true, 1)?;
    store.0.run(|c| {
        assert_eq!(
            c.query_one("SELECT count(*) FROM foursquare_import_fingerprints", &[])?
                .get::<_, i64>(0),
            0
        );
        Ok(())
    })?;
    store.reconcile_import(vec![input.clone()], false, 1)?;
    input.version = Some("a-new-export-of-the-same-place".into());
    store.0.run(move |c| {
        let mut tx = c.transaction()?;
        // The normal reconciliation path requires this relation even when all
        // places already exist. An identical fingerprint must bypass it.
        tx.batch_execute("ALTER TABLE merchant_identity_keys RENAME TO unavailable_identity_keys")?;
        let cached = super::import(&mut tx, std::slice::from_ref(&input))?;
        assert_eq!(cached.0.unchanged, 1);
        assert!(cached.1.is_empty());
        tx.batch_execute("SAVEPOINT changed_input")?;
        input.raw["places"][0]["address"] = serde_json::json!("Changed Street");
        assert!(
            super::import(&mut tx, &[input]).is_err(),
            "changed input must reconcile"
        );
        tx.batch_execute("ROLLBACK TO SAVEPOINT changed_input")?;
        tx.rollback()?;
        Ok(())
    })?;
    Ok(())
}

#[test]
fn fingerprint_invalidation_follows_links_manual_edits_and_place_categories() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let input = records()?.remove(0);
    store.reconcile_import(vec![input.clone()], false, 1)?;
    let owner = store.resolve_source("foursquare", "place:1")?.unwrap();
    let cache_count = |store: &MerchantStore| {
        store.0.run(|c| {
            Ok(
                c.query_one("SELECT count(*) FROM foursquare_import_fingerprints", &[])?
                    .get::<_, i64>(0),
            )
        })
    };
    assert_eq!(cache_count(&store)?, 1);
    store.put(&store.get(&owner)?.unwrap())?;
    assert_eq!(cache_count(&store)?, 0);
    store.reconcile_import(vec![input.clone()], false, 1)?;
    assert_eq!(cache_count(&store)?, 1);
    store.0.run(|c| {
        c.execute(
            "DELETE FROM foursquare_place_categories WHERE external_id='place:1'",
            &[],
        )?;
        Ok(())
    })?;
    assert_eq!(cache_count(&store)?, 0);
    store.reconcile_import(vec![input.clone()], false, 1)?;
    let target: Merchant =
        serde_json::from_value(serde_json::json!({"id":"cache-link-target","name":"Target"}))?;
    store.put(&target)?;
    store.link("foursquare", "place:1", &target.id)?;
    assert_eq!(cache_count(&store)?, 0);
    store.reconcile_import(vec![input.clone()], false, 1)?;
    assert_eq!(
        store.resolve_source("foursquare", "place:1")?,
        Some(target.id)
    );
    assert_eq!(cache_count(&store)?, 1);
    let mut merchants_only = input;
    merchants_only.raw.as_object_mut().unwrap().remove("places");
    store.reconcile_import(vec![merchants_only.clone()], true, 1)?;
    // A preview may neither replace the full-mode fingerprint nor its data.
    store.0.run(move |c| {
        let saved: Vec<u8> = c.query_one("SELECT fingerprint FROM foursquare_import_fingerprints WHERE external_id='place:1'", &[])?.get(0);
        assert_ne!(saved, fingerprint(&merchants_only)?);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn fingerprint_migration_keeps_catalog_and_warms_on_first_replay() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let input = records()?.remove(0);
    store.reconcile_import(vec![input.clone()], false, 1)?;
    let id = store.resolve_source("foursquare", "place:1")?.unwrap();
    let before = store.get(&id)?.unwrap();
    store.0.run(|c| {
        c.batch_execute("DROP FUNCTION invalidate_foursquare_import_fingerprints() CASCADE; DROP TABLE foursquare_import_fingerprints; UPDATE ultrafinance_schema SET version=29")?;
        Ok(())
    })?;
    let upgraded = MerchantStore::initialize_postgres(store.temporary_url())?;
    assert_eq!(upgraded.get(&id)?.unwrap(), before);
    let replay = upgraded.reconcile_import(vec![input], false, 1)?;
    assert_eq!(replay.delta.unchanged, 1);
    upgraded.0.run(|c| {
        assert_eq!(
            c.query_one("SELECT count(*) FROM foursquare_import_fingerprints", &[])?
                .get::<_, i64>(0),
            1
        );
        Ok(())
    })?;
    Ok(())
}

#[test]
fn fingerprint_mixed_chunk_skips_hits_and_invalidates_affected_siblings() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let mut inputs = records()?;
    store.reconcile_import(inputs.clone(), false, 2)?;
    inputs[1].raw["places"][0]["address"] = serde_json::json!("2 New Street");
    let changed = store.reconcile_import(inputs.clone(), false, 2)?;
    assert_eq!(changed.delta.updated, 1);
    assert_eq!(changed.delta.unchanged, 1);
    store.0.run(|c| {
        // Updating a sibling's ownership/location invalidates old owner cache
        // entries; only the place actually reconciled is marked current again.
        assert_eq!(
            c.query_one("SELECT count(*) FROM foursquare_import_fingerprints", &[])?
                .get::<_, i64>(0),
            1
        );
        Ok(())
    })?;
    let replay = store.reconcile_import(inputs, false, 2)?;
    assert_eq!(replay.delta.unchanged, 2);
    assert_eq!(replay.delta.updated, 0);
    Ok(())
}

#[test]
fn owner_lookup_deduplicates_merchants_and_counts_all_provider_identities() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let mut inputs = records()?;
    let mut independent = inputs[0].clone();
    independent.external_id = "place:independent".into();
    independent.raw = serde_json::json!({}); // Addressless identity.
    independent.merchant.name = "Independent Merchant".into();
    independent.merchant.website = Some("https://independent.test".into());
    inputs.push(independent.clone());
    store.reconcile_import(inputs, false, 3)?;
    let shared = store.resolve_source("foursquare", "place:1")?.unwrap();
    let single = store
        .resolve_source("foursquare", "place:independent")?
        .unwrap();
    let keys = vec![
        "place:1".into(),
        "place:2".into(),
        "place:independent".into(),
        "missing".into(),
    ];
    let load = |store: &MerchantStore| {
        let keys = keys.clone();
        store.0.run(move |c| {
            let mut tx = c.transaction()?;
            let mut state = OwnerState::default();
            state.load(
                &mut tx,
                include_str!("../../../src/postgres_store/foursquare_identity_lookup.sql"),
                &keys,
            )?;
            tx.rollback()?;
            Ok(state)
        })
    };
    let state = load(&store)?;
    assert_eq!(state.old.len(), 3);
    assert_eq!(state.before.len(), 2);
    assert_eq!(state.numeric.len(), 2);
    assert_eq!(state.old["place:1"], shared);
    assert_eq!(state.old["place:2"], shared);
    assert!(!state.replaceable.contains(&shared));
    assert!(state.replaceable.contains(&single));
    assert!(state.manual.is_empty());
    store.put(&store.get(&single)?.unwrap())?;
    assert!(load(&store)?.manual.contains(&single));
    independent.source = "other-provider".into();
    independent.external_id = "other-id".into();
    store.reconcile_import(vec![independent], false, 1)?;
    assert!(!load(&store)?.replaceable.contains(&single));
    Ok(())
}

#[test]
fn compact_identity_replays_and_addressless_transition_keep_ids() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let inputs = records()?;
    let first = store.reconcile_import(inputs.clone(), false, 1)?;
    assert_eq!(first.delta.added, 2);
    let id = store.resolve_source("foursquare", "place:1")?.unwrap();
    let outlet = store.locations(&id)?.remove(0);
    assert_eq!(
        store.resolve_source("foursquare", "place:2")?,
        Some(id.clone())
    );
    store.0.run(|c| {
        assert_eq!(
            c.query_one(
                "SELECT count(*) FROM source_records WHERE source='foursquare'",
                &[]
            )?
            .get::<_, i64>(0),
            0
        );
        assert_eq!(
            c.query_one("SELECT count(*) FROM foursquare_unlocated", &[])?
                .get::<_, i64>(0),
            1
        );
        assert_eq!(
            c.query_one(
                "SELECT count(*) FROM location_records WHERE merchant_source IS NOT NULL",
                &[]
            )?
            .get::<_, i64>(0),
            0
        );
        Ok(())
    })?;
    let replay = store.reconcile_import(inputs.clone(), false, 1)?;
    assert_eq!(replay.delta.unchanged, 2);
    assert_eq!(store.locations(&id)?[0].location.id, outlet.location.id);
    let mut addressed = inputs[1].clone();
    addressed.raw["places"][0]["address"] = serde_json::json!("2 Main St");
    store.reconcile_import(vec![addressed], false, 1)?;
    assert_eq!(
        store.resolve_source("foursquare", "place:2")?,
        Some(id.clone())
    );
    assert_eq!(store.locations(&id)?.len(), 2);
    store.0.run(|c| {
        assert_eq!(
            c.query_one("SELECT count(*) FROM foursquare_unlocated", &[])?
                .get::<_, i64>(0),
            0
        );
        assert_eq!(
            c.query_one(
                "SELECT count(*) FROM source_identities WHERE source='foursquare'",
                &[]
            )?
            .get::<_, i64>(0),
            2
        );
        Ok(())
    })?;
    Ok(())
}

#[test]
fn migration_removes_foursquare_evidence_and_preserves_import_identity() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let inputs = records()?;
    store.reconcile_import(inputs.clone(), false, 5000)?;
    let merchant = store.resolve_source("foursquare", "place:1")?.unwrap();
    let outlet = store.locations(&merchant)?[0].location.id.clone();
    let owner = merchant.clone();
    store.0.run(move |c| {
        let mut tx=c.transaction()?;
        tx.batch_execute(include_str!("version_18_public_ids.sql"))?;
        tx.batch_execute(include_str!("version_28_categories.sql"))?;
        tx.batch_execute("DROP VIEW source_identities; DROP TABLE foursquare_unlocated; ALTER TABLE source_records DROP CONSTRAINT source_records_no_foursquare; UPDATE ultrafinance_schema SET version=17")?;
        for record in inputs {
            // Write the historical shape directly: current writers require schema 29.
            let provenance: i64 = tx.query_one("INSERT INTO source_provenance(source,attribution,license,url) VALUES($1,$2,$3,$4) ON CONFLICT(source,attribution,license,url) DO UPDATE SET source=excluded.source RETURNING id", &[&record.source,&record.attribution,&record.license,&record.url])?.get(0);
            tx.execute("INSERT INTO source_records(source,external_id,merchant_id,source_provenance_id,input_id,input_name,input_website,input_markets,input_aliases,input_sources,input_category_ids) VALUES($1,$2,$3,$4,$2,$5,$6,$7,$8,$9,$10)", &[&record.source,&record.external_id,&owner,&provenance,&record.merchant.name,&record.merchant.website,&record.merchant.markets,&record.merchant.aliases,&record.merchant.sources,&record.merchant.category_ids])?;
        }
        tx.batch_execute("UPDATE location_records SET merchant_source='foursquare',merchant_external_id=external_id WHERE source='foursquare'")?;
        tx.commit()?;
        Ok(())
    })?;
    let migrated = PostgresStore::connect(store.temporary_url(), true)?;
    assert_eq!(
        migrated.resolve_source("foursquare", "place:1")?,
        Some(merchant.clone())
    );
    assert_eq!(
        migrated.resolve_source("foursquare", "place:2")?,
        Some(merchant.clone())
    );
    assert_eq!(migrated.locations(&merchant)?[0].location.id, outlet);
    migrated.run(|c| {
        assert_eq!(
            c.query_one("SELECT count(*) FROM source_records", &[])?
                .get::<_, i64>(0),
            0
        );
        assert_eq!(
            c.query_one("SELECT count(*) FROM foursquare_unlocated", &[])?
                .get::<_, i64>(0),
            1
        );
        assert_eq!(
            c.query_one("SELECT version FROM ultrafinance_schema", &[])?
                .get::<_, i32>(0),
            crate::store::POSTGRES_SCHEMA_VERSION
        );
        Ok(())
    })?;
    Ok(())
}

#[test]
fn addressless_links_manual_corrections_and_preview_survive_reimport() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let inputs = records()?;
    store.reconcile_import(inputs.clone(), false, 5000)?;
    let mut target = inputs[0].merchant.clone();
    target.id = "reviewed-target".into();
    target.name = "Reviewed Cafe".into();
    store.put(&target)?;
    store.link("foursquare", "place:2", &target.id)?;
    let mut addressed = inputs[1].clone();
    addressed.raw["places"][0]["address"] = serde_json::json!("New Street");
    store.reconcile_import(vec![addressed.clone()], true, 5000)?;
    assert!(store.locations(&target.id)?.is_empty());
    assert_eq!(
        store.resolve_source("foursquare", "place:2")?,
        Some(target.id.clone())
    );
    store.reconcile_import(vec![addressed.clone()], false, 5000)?;
    assert_eq!(store.get(&target.id)?.unwrap().name, "Reviewed Cafe");
    assert_eq!(store.locations(&target.id)?.len(), 1);
    assert_eq!(
        store
            .reconcile_import(vec![addressed], false, 5000)?
            .delta
            .unchanged,
        1
    );
    let empty = MerchantStore::temporary()?;
    let duplicate = inputs[1].clone();
    assert!(
        empty
            .initial_foursquare_load(2, false, 1, move |consume| {
                consume(vec![duplicate.clone()])?;
                consume(vec![duplicate])
            })
            .is_err()
    );
    assert_eq!(empty.stats()?.total, 0);
    Ok(())
}

#[test]
fn incremental_duplicate_reports_stay_bounded_without_losing_counts() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let template = records()?.remove(1);
    let inputs: Vec<_> = (0..6002)
        .map(|i| {
            let mut r = template.clone();
            r.external_id = format!("place:{i}");
            r.merchant.id = r.external_id.clone();
            r.raw = serde_json::json!({});
            r
        })
        .collect();
    let report = store.reconcile_import(inputs, false, 5000)?;
    assert_eq!(report.delta.added, 6002);
    assert_eq!(report.dedupe.candidates, 6001);
    assert!(report.dedupe.details_truncated);
    assert!(report.dedupe.groups.len() <= 5000);
    assert_eq!(store.stats()?.total, 1);
    Ok(())
}

#[test]
fn categories_survive_legacy_import_refresh_and_single_place_replacement() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let mut record = records()?.remove(0);
    record.raw["places"][0]["fsq_category_ids"] = serde_json::json!(["5109983191d435c0d71c2bb1"]);
    record.merchant.category_ids.clear(); // Old prepared bundle format.
    store.reconcile_import(vec![record.clone()], false, 1)?;
    let id = store
        .resolve_source("foursquare", &record.external_id)?
        .unwrap();
    assert_eq!(
        store.get(&id)?.unwrap().category_ids,
        ["5109983191d435c0d71c2bb1"]
    );
    record.merchant.category_ids = vec!["new-category".into()];
    store.reconcile_import(vec![record.clone()], true, 1)?;
    assert_eq!(
        store.get(&id)?.unwrap().category_ids,
        ["5109983191d435c0d71c2bb1"]
    );
    store.reconcile_import(vec![record], false, 1)?;
    assert_eq!(
        store.get(&id)?.unwrap().category_ids,
        ["5109983191d435c0d71c2bb1"]
    );
    let reopened = MerchantStore::postgres(store.temporary_url())?;
    assert_eq!(
        reopened.get(&id)?.unwrap().category_ids,
        ["5109983191d435c0d71c2bb1"]
    );
    Ok(())
}

#[test]
fn initial_load_keeps_first_place_categories_across_chunks() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let mut records = records()?;
    records[0].merchant.category_ids = vec!["a".into(), "b".into()];
    records[1].merchant.category_ids = vec!["b".into(), "c".into()];
    let inputs = records.clone();
    store.initial_foursquare_load(2, false, 1, move |consume| consume(inputs))?;
    let id = store
        .resolve_source("foursquare", &records[0].external_id)?
        .unwrap();
    assert_eq!(store.get(&id)?.unwrap().category_ids, ["a", "b"]);
    store.reconcile_import(records, false, 1)?;
    assert_eq!(store.get(&id)?.unwrap().category_ids, ["a", "b"]);
    Ok(())
}

fn projection_versions(store: &MerchantStore) -> Result<serde_json::Value> {
    store.0.run(|c| {
        Ok(serde_json::json!({
            "aliases":c.query("SELECT merchant_id,normalized,xmin::text FROM aliases ORDER BY merchant_id,normalized", &[])?.iter().map(|r| (r.get::<_,i64>(0),r.get::<_,String>(1),r.get::<_,String>(2))).collect::<Vec<_>>(),
            "search":c.query("SELECT merchant_id,text,xmin::text FROM merchant_search ORDER BY merchant_id", &[])?.iter().map(|r| (r.get::<_,i64>(0),r.get::<_,String>(1),r.get::<_,String>(2))).collect::<Vec<_>>(),
            "identity":c.query("SELECT merchant_id,normalized_name,website_host,xmin::text FROM merchant_identity_keys ORDER BY merchant_id", &[])?.iter().map(|r| (r.get::<_,i64>(0),r.get::<_,String>(1),r.get::<_,Option<String>>(2),r.get::<_,String>(3))).collect::<Vec<_>>(),
            "locations":c.query("SELECT external_id,xmin::text FROM location_records ORDER BY external_id", &[])?.iter().map(|r| (r.get::<_,String>(0),r.get::<_,String>(1))).collect::<Vec<_>>()
        }))
    })
}

#[test]
fn replay_and_metadata_deltas_only_touch_dependent_projections() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let mut input = records()?.remove(0);
    store.reconcile_import(vec![input.clone()], false, 1)?;
    let baseline = projection_versions(&store)?;
    let revision = store.0.run(|c| {
        Ok(c.query_one("SELECT revision FROM catalog_revision", &[])?
            .get::<_, i64>(0))
    })?;
    let replay = store.reconcile_import(vec![input.clone()], false, 1)?;
    assert_eq!(replay.delta.unchanged, 1);
    assert_eq!(projection_versions(&store)?, baseline);
    assert_eq!(
        store.0.run(|c| Ok(c
            .query_one("SELECT revision FROM catalog_revision", &[])?
            .get::<_, i64>(0)))?,
        revision
    );
    input.merchant.category_ids = vec!["replacement".into()];
    let report = store.reconcile_import(vec![input.clone()], false, 1)?;
    assert_eq!(report.delta.updated, 1);
    assert_eq!(
        projection_versions(&store)?,
        baseline,
        "category edits must not touch any matching projection or outlet"
    );
    input.merchant.website = Some("https://changed.example".into());
    store.reconcile_import(vec![input.clone()], false, 1)?;
    let website = projection_versions(&store)?;
    assert_eq!(website["aliases"], baseline["aliases"]);
    assert_eq!(website["search"], baseline["search"]);
    assert_eq!(website["locations"], baseline["locations"]);
    assert_ne!(website["identity"], baseline["identity"]);
    input.raw["places"][0]["address"] = serde_json::json!("Changed Street");
    store.reconcile_import(vec![input.clone()], false, 1)?;
    let address = projection_versions(&store)?;
    assert_eq!(address["aliases"], website["aliases"]);
    assert_eq!(address["search"], website["search"]);
    assert_eq!(address["identity"], website["identity"]);
    assert_ne!(address["locations"], website["locations"]);
    input.merchant.name = "Renamed Cafe".into();
    store.reconcile_import(vec![input], false, 1)?;
    let renamed = projection_versions(&store)?;
    assert_ne!(renamed["search"], address["search"]);
    assert_ne!(renamed["identity"], address["identity"]);
    Ok(())
}

#[test]
fn place_category_updates_do_not_change_created_merchant_categories() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let mut inputs = records()?;
    inputs[0].merchant.category_ids = vec!["a".into(), "shared".into()];
    inputs[1].merchant.category_ids = vec!["b".into(), "shared".into()];
    // Both places resolve to one merchant within the same chunk.
    store.reconcile_import(inputs.clone(), false, 2)?;
    let id = store.resolve_source("foursquare", "place:1")?.unwrap();
    assert_eq!(store.get(&id)?.unwrap().category_ids, ["a", "shared"]);
    for input in &mut inputs {
        input.merchant.category_ids = vec!["replacement".into()];
    }
    store.reconcile_import(inputs.clone(), false, 1)?;
    assert_eq!(store.get(&id)?.unwrap().category_ids, ["a", "shared"]);
    store.0.run(|c| {
        assert_eq!(
            c.query_one("SELECT foursquare_category_ids('place:1')", &[])?
                .get::<_, Vec<String>>(0),
            ["replacement"]
        );
        Ok(())
    })?;
    let replay = store.reconcile_import(inputs.clone(), false, 2)?;
    assert_eq!(replay.delta.unchanged, 2);
    let target: Merchant = serde_json::from_value(serde_json::json!({
        "id":"reviewed-category-target","name":"Reviewed Target","category_ids":["manual"]
    }))?;
    store.put(&target)?;
    store.link("foursquare", "place:1", &target.id)?;
    assert_eq!(store.get(&target.id)?.unwrap().category_ids, ["manual"]);
    store.reconcile_import(vec![inputs.remove(0)], false, 1)?;
    assert_eq!(store.get(&target.id)?.unwrap().category_ids, ["manual"]);
    assert_eq!(store.get(&id)?.unwrap().category_ids, ["a", "shared"]);
    Ok(())
}

#[test]
fn place_category_changes_preserve_merged_merchant_and_other_sources() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let mut inputs = records()?;
    inputs[0].merchant.category_ids = vec!["old".into()];
    inputs[1].merchant.name = "Different Cafe".into();
    inputs[1].merchant.website = Some("https://different.example".into());
    inputs[1].merchant.category_ids = vec!["keep".into()];
    store.reconcile_import(inputs.clone(), false, 1)?;
    let target = store
        .resolve_source("foursquare", &inputs[0].external_id)?
        .unwrap();
    let retired = store
        .resolve_source("foursquare", &inputs[1].external_id)?
        .unwrap();
    let group = vec![target.clone(), retired];
    store.0.run(move |c| {
        let mut tx = c.transaction()?;
        merge_groups(&mut tx, &[group])?;
        tx.commit()?;
        Ok(())
    })?;
    assert_eq!(store.get(&target)?.unwrap().category_ids, ["keep", "old"]);
    let mut other = inputs[0].clone();
    other.source = "fixture-categories".into();
    other.external_id = "external".into();
    other.merchant.category_ids = vec!["old".into()];
    other.raw = serde_json::json!({});
    store.reconcile_import(vec![other], false, 1)?;
    inputs[0].merchant.category_ids = vec!["replacement".into()];
    store.reconcile_import(vec![inputs[0].clone()], false, 1)?;
    assert_eq!(store.get(&target)?.unwrap().category_ids, ["keep", "old"]);
    // Clearing the other source must finally remove the obsolete assignment.
    let mut other = inputs[0].clone();
    other.source = "fixture-categories".into();
    other.external_id = "external".into();
    other.merchant.category_ids = vec![];
    other.raw = serde_json::json!({});
    store.reconcile_import(vec![other], false, 1)?;
    assert_eq!(store.get(&target)?.unwrap().category_ids, ["keep", "old"]);
    Ok(())
}

#[test]
fn provider_id_normalization_preserves_source_exports_and_location_identity() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let input = records()?.remove(0);
    store.reconcile_import(vec![input.clone()], false, 1)?;
    let id = store
        .resolve_source("foursquare", &input.external_id)?
        .unwrap();
    let before = store.locations(&id)?;
    let mut source = input.clone();
    source.source = "identifier-fixture".into();
    source.external_id = "same-input-id".into();
    source.merchant.id = source.external_id.clone();
    source.raw = serde_json::json!({});
    store.reconcile_import(vec![source.clone()], false, 1)?;
    let original = store.source_records("identifier-fixture", Some("same-input-id"), 10, 0)?;
    let key = input.external_id.clone();
    store.0.run(move |c| {
        assert!(c.query_one("SELECT input_id IS NULL FROM source_records WHERE source='identifier-fixture'", &[])?.get::<_,bool>(0));
        assert!(c.query_one("SELECT identity_external_id IS NULL FROM foursquare_categories WHERE external_id=$1", &[&key])?.get::<_,bool>(0));
        // Reconstruct the redundant version-27 values to exercise the real upgrade.
        c.batch_execute(include_str!("version_28_categories.sql"))?;
        c.batch_execute("ALTER TABLE location_records ADD COLUMN place_ids JSONB NOT NULL DEFAULT '{}'; UPDATE location_records SET place_ids=jsonb_build_object('foursquare',external_id); UPDATE source_records SET input_id=external_id WHERE input_id IS NULL; UPDATE foursquare_categories SET identity_external_id=external_id WHERE identity_external_id IS NULL; UPDATE ultrafinance_schema SET version=27")?;
        Ok(())
    })?;
    let upgraded = MerchantStore::initialize_postgres(store.temporary_url())?;
    assert_eq!(
        serde_json::to_value(upgraded.locations(&id)?)?,
        serde_json::to_value(before)?
    );
    assert_eq!(
        upgraded.source_records("identifier-fixture", Some("same-input-id"), 10, 0)?,
        original
    );
    upgraded.0.run(|c| {
        assert!(!c.query_one("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='location_records' AND column_name='place_ids')", &[])?.get::<_,bool>(0));
        Ok(())
    })?;
    // A genuinely distinct input object ID remains round-trippable on updates.
    source.merchant.id = "distinct-original-object".into();
    upgraded.reconcile_import(vec![source], false, 1)?;
    let exports = upgraded.source_records("identifier-fixture", Some("same-input-id"), 10, 0)?;
    assert_eq!(
        exports[0]["record"]["merchant"]["id"],
        "distinct-original-object"
    );
    Ok(())
}

#[test]
fn database_taxonomy_resolves_names_hierarchy_and_keeps_integer_ids() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let mut input = records()?.remove(0);
    input.merchant.category_ids = vec!["5109983191d435c0d71c2bb1".into(), "future-category".into()];
    store.reconcile_import(vec![input.clone()], false, 1)?;
    let id = store
        .resolve_source("foursquare", &input.external_id)?
        .unwrap();
    let merchant = store.get(&id)?.unwrap();
    let details = merchant.categories.unwrap();
    assert_eq!(details[0].name.as_deref(), Some("Attraction"));
    assert_eq!(
        details[0]
            .path
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["Arts and Entertainment", "Amusement Park", "Attraction"]
    );
    assert_eq!(details[1].id, "future-category");
    assert!(details[1].name.is_none());
    let category_id: i32 = store.0.run(|c| {
        assert_eq!(c.query_one("SELECT count(*) FROM categories WHERE name IS NOT NULL", &[])?.get::<_,i64>(0),1279);
        let ids: Vec<String> = c.query("SELECT external_id FROM categories WHERE name IS NOT NULL ORDER BY external_id", &[])?.iter().map(|r| r.get(0)).collect();
        let details: String = c.query_one("SELECT category_details($1)::text", &[&ids])?.get(0);
        let seeded: Vec<crate::categories::Category> = serde_json::from_str(&details)?;
        assert_eq!(seeded, crate::categories::resolve(&ids));
        // Assignments carry integer foreign keys, never repeated provider IDs.
        assert_eq!(c.query_one("SELECT count(*) FROM information_schema.columns WHERE table_schema=current_schema() AND table_name IN ('merchants','manual_merchants','source_records','foursquare_categories') AND column_name IN ('category_ids','input_category_ids')", &[])?.get::<_,i64>(0),0);
        let id: i32 = c.query_one("SELECT id FROM categories WHERE external_id='5109983191d435c0d71c2bb1'", &[])?.get(0);
        let revision: i64 = c.query_one("SELECT revision FROM catalog_revision", &[])?.get(0);
        c.execute("UPDATE categories SET name='Updated Attraction',parent_id=(SELECT id FROM categories WHERE external_id='4d4b7104d754a06370d81259') WHERE id=$1", &[&id])?;
        assert!(c.query_one("SELECT revision FROM catalog_revision", &[])?.get::<_,i64>(0)>revision);
        let error = c.execute("INSERT INTO foursquare_place_categories(external_id,category_id) VALUES('place:1',-1)", &[]).unwrap_err();
        assert_eq!(error.code(),Some(&postgres::error::SqlState::FOREIGN_KEY_VIOLATION));
        Ok(id)
    })?;
    let updated = store.get(&id)?.unwrap().categories.unwrap();
    assert_eq!(updated[0].name.as_deref(), Some("Updated Attraction"));
    assert_eq!(updated[0].path.len(), 2);
    // Updating assignments does not overwrite locally maintained taxonomy data.
    store.reconcile_import(vec![input], false, 1)?;
    store.0.run(move |c| {
        let row = c.query_one(
            "SELECT id,name FROM categories WHERE external_id='5109983191d435c0d71c2bb1'",
            &[],
        )?;
        assert_eq!(row.get::<_, i32>(0), category_id);
        assert_eq!(row.get::<_, String>(1), "Updated Attraction");
        Ok(())
    })?;
    Ok(())
}

#[test]
fn category_table_migration_preserves_all_evidence_without_reimport() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let mut inputs = records()?;
    inputs[0].merchant.category_ids =
        vec!["5109983191d435c0d71c2bb1".into(), "legacy-unknown".into()];
    inputs[1].merchant.category_ids = vec!["4d4b7104d754a06370d81259".into()];
    store.reconcile_import(inputs.clone(), false, 1)?;
    let id = store
        .resolve_source("foursquare", &inputs[0].external_id)?
        .unwrap();
    let mut other = inputs[0].clone();
    other.source = "category-migration-source".into();
    other.external_id = "original".into();
    other.merchant.category_ids = vec!["other-source".into()];
    other.raw = serde_json::json!({});
    store.reconcile_import(vec![other], false, 1)?;
    let mut manual = store.get(&id)?.unwrap();
    manual.category_ids = vec!["manual-category".into()];
    store.put(&manual)?;
    let before = store.get(&id)?.unwrap();
    let exports = store.source_records("category-migration-source", None, 10, 0)?;
    store.0.run(|c| {
        c.batch_execute(include_str!("version_28_categories.sql"))?;
        c.batch_execute("UPDATE ultrafinance_schema SET version=28")?;
        Ok(())
    })?;
    let upgraded = MerchantStore::initialize_postgres(store.temporary_url())?;
    assert_eq!(upgraded.get(&id)?.unwrap(), before);
    assert_eq!(
        upgraded.source_records("category-migration-source", None, 10, 0)?,
        exports
    );
    inputs[0].merchant.category_ids.clear();
    upgraded.reconcile_import(vec![inputs[0].clone()], false, 1)?;
    let categories = upgraded.get(&id)?.unwrap().category_ids;
    assert!(categories.contains(&"legacy-unknown".into()));
    assert!(categories.contains(&"manual-category".into()));
    assert!(categories.contains(&"other-source".into()));
    Ok(())
}
