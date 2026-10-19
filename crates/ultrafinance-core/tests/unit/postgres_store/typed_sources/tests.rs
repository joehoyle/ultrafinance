use super::*;
#[test]
fn version_four_backfill_retains_documents_and_projects_typed_inputs() -> Result<()> {
    let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
    let store = PostgresStore::temporary(&base)?;
    store.run(|client| {
        let mut tx=client.transaction()?;
        tx.batch_execute(concat!(include_str!("../version_18_public_ids.sql"), include_str!("../version_16_source_metadata.sql")))?;
        // Reconstruct a populated version-4 source table without changing
        // the caller's catalog; this entire fixture is disposable.
        tx.batch_execute("DROP VIEW IF EXISTS source_records_documents; ALTER TABLE source_records ADD COLUMN merchant_json TEXT,ADD COLUMN raw_json TEXT; ALTER TABLE source_records DROP COLUMN input_id,DROP COLUMN input_name,DROP COLUMN input_website,DROP COLUMN input_logo_url,DROP COLUMN input_logo_source,DROP COLUMN input_markets,DROP COLUMN input_aliases,DROP COLUMN input_sources,DROP COLUMN country_hints,DROP COLUMN dataset_region,DROP COLUMN negative_aliases; UPDATE ultrafinance_schema SET version=4")?;
        let merchant: Merchant=serde_json::from_value(serde_json::json!({"id":"input","name":"Café 東京","markets":["CA"],"aliases":["BANK CAFE"]}))?;
        tx.execute("INSERT INTO merchants(id,name) VALUES('canonical',$1)", &[&merchant.name])?;
        let raw=serde_json::json!({"countryHints":["CA",null,5,"US"],"payload":"x".repeat(128*1024)});
        let merchant_doc=serde_json::to_string(&merchant)?;let raw_doc=serde_json::to_string(&raw)?;
        tx.execute("INSERT INTO source_records(source,external_id,merchant_id,merchant_json,raw_json,version,attribution,license,url) VALUES('open-enrichment','external','canonical',$1,$2,'fnv1a64:abc:ca:','attribution','license','https://example.org')",&[&merchant_doc,&raw_doc])?;
        tx.batch_execute(include_str!("../../../../migrations/006_typed_source_inputs.sql"))?;
        let inputs:Vec<_>=tx.query("SELECT merchant_id,input_id,input_name,input_website,input_logo_url,input_logo_source,input_markets,input_aliases,input_sources,url,country_hints,dataset_region,input_counterparty_type FROM source_records WHERE merchant_id='canonical'", &[])?.iter().map(decode).collect::<Result<Vec<_>>>()?;
        assert_eq!(inputs.len(),1);
        assert_eq!(serde_json::to_value(&inputs[0].merchant)?,serde_json::to_value(&merchant)?);
        assert_eq!(inputs[0].hints,vec!["CA","US"]);
        assert_eq!(inputs[0].region.as_deref(),Some("ca"));
        let stored=tx.query_one("SELECT merchant_json,raw_json FROM source_records",&[])?;
        assert_eq!(stored.get::<_,String>(0),merchant_doc);
        assert_eq!(stored.get::<_,String>(1),raw_doc);
        tx.batch_execute("CREATE VIEW source_records_documents AS SELECT merchant_json AS data FROM source_records")?;
        tx.batch_execute(include_str!("../../../../migrations/007_source_inputs_only.sql"))?;
        let doc:String=tx.query_one("SELECT data FROM source_records_documents",&[])?.get(0);
        let compact:SourceRecord=serde_json::from_str(&doc)?;
        assert_eq!(compact.raw,serde_json::json!({"countryHints":["CA","US"]}));
        assert_eq!(serde_json::to_value(compact.merchant)?,serde_json::to_value(&merchant)?);
        tx.execute("DELETE FROM source_records",&[])?;
        assert!(tx.query_one("SELECT to_regclass('source_market_inputs') IS NULL", &[])?.get::<_,bool>(0));
        tx.commit()?;
        Ok(())
    })
}
