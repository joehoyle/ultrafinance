use super::*;
use crate::store::MerchantStore;

#[test]
fn migration_keeps_public_ids_outlets_and_search_and_allocates_fresh_keys() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let records = crate::foursquare::prepare(
        r#"fsq_place_id,name,country,address,website,fsq_category_ids,date_closed
1,Cafe,CA,1 Main St,https://cafe.test,["shop"],
2,Cafe,CA,,https://cafe.test,["shop"],
"#,
        "ca",
    )?
    .0;
    store.reconcile_import(records.clone(), false, 1)?;
    let id = store.resolve_source("foursquare", "place:1")?.unwrap();
    let merchant = store.get(&id)?.unwrap();
    let outlets = serde_json::to_value(store.location_sources(&id)?)?;
    store.0.run(|c| {
        let mut tx = c.transaction()?;
        tx.batch_execute(include_str!("version_18_public_ids.sql"))?;
        tx.batch_execute("GRANT SELECT ON source_identities TO PUBLIC; GRANT INSERT ON merchants TO PUBLIC; ALTER TABLE location_records ENABLE ALWAYS TRIGGER location_merchant")?;
        tx.commit()?;
        Ok(())
    })?;
    let upgraded = PostgresStore::connect(store.temporary_url(), true)?;
    assert_eq!(
        serde_json::to_value(upgraded.get(&id)?)?,
        serde_json::to_value(Some(&merchant))?
    );
    assert_eq!(
        serde_json::to_value(upgraded.location_sources(&id)?)?,
        outlets
    );
    assert_eq!(upgraded.search("Cafe", Some("CA"), 10)?[0].merchant.id, id);
    for place in ["place:1", "place:2"] {
        assert_eq!(
            upgraded.resolve_source("foursquare", place)?,
            Some(id.clone())
        );
    }
    let original = id.clone();
    let key: i64 = upgraded.run(move |c| {
        assert_eq!(c.query_one("SELECT version FROM ultrafinance_schema",&[])?.get::<_,i32>(0),crate::store::POSTGRES_SCHEMA_VERSION);
        assert_eq!(c.query_one("SELECT tgenabled::text FROM pg_trigger WHERE tgrelid='location_records'::regclass AND tgname='location_merchant'",&[])?.get::<_,String>(0),"A");
        assert!(c.query_one("SELECT EXISTS(SELECT 1 FROM information_schema.table_privileges WHERE table_schema='public' AND table_name='source_identities' AND grantee='PUBLIC' AND privilege_type='SELECT')", &[])?.get::<_, bool>(0));
        assert!(c.query_one("SELECT EXISTS(SELECT 1 FROM pg_class c CROSS JOIN LATERAL aclexplode(c.relacl) a WHERE c.oid='merchants_merchant_key_seq'::regclass AND a.grantee=0 AND a.privilege_type='USAGE')", &[])?.get::<_, bool>(0));
        for table in ["aliases","merchant_search","merchant_identity_keys","location_records","foursquare_unlocated"] {
            assert_eq!(c.query_one("SELECT data_type FROM information_schema.columns WHERE table_schema='public' AND table_name=$1 AND column_name='merchant_id'",&[&table])?.get::<_,String>(0),"bigint");
        }
        Ok(c.query_one("SELECT merchant_key FROM merchants WHERE id=$1",&[&original])?.get(0))
    })?;
    // Incoming refreshes retain both IDs; a newly created public ID receives a
    // distinct key from the migrated identity sequence.
    upgraded.reconcile_import(records, false, 1)?;
    let mut other = merchant.clone();
    other.id = "manual-public-id".into();
    other.name = "Different shop".into();
    other.website = None;
    upgraded.put(&other)?;
    upgraded.run(move |c| {
        let existing: i64 = c
            .query_one("SELECT merchant_key FROM merchants WHERE id=$1", &[&id])?
            .get(0);
        let fresh: i64 = c
            .query_one(
                "SELECT merchant_key FROM merchants WHERE id='manual-public-id'",
                &[],
            )?
            .get(0);
        assert_eq!(existing, key);
        assert_ne!(fresh, key);
        Ok(())
    })
}
