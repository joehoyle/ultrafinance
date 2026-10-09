//! Typed, compact source inputs for canonical and coverage rebuilding.
use super::*;
pub(super) struct Input {
    pub merchant_id: String,
    pub merchant: Merchant,
    pub url: String,
    pub hints: Vec<String>,
    pub region: Option<String>,
}
fn decode(r: &postgres::Row) -> Input {
    Input {
        merchant_id: r.get(0),
        merchant: Merchant {
            id: r.get(1),
            name: r.get(2),
            website: r.get(3),
            logo_url: r.get(4),
            logo_source: r.get(5),
            markets: r.get(6),
            aliases: r.get(7),
            sources: r.get(8),
        },
        url: r.get(9),
        hints: r.get(10),
        region: r.get(11),
    }
}
pub(super) fn read(c: &mut impl GenericClient, ids: Option<&[String]>) -> Result<Vec<Input>> {
    Ok(c.query("SELECT merchant_id,input_id,input_name,input_website,input_logo_url,input_logo_source,input_markets,input_aliases,input_sources,url,country_hints,dataset_region FROM source_records WHERE ($1::text[] IS NULL OR merchant_id=ANY($1)) ORDER BY source,external_id", &[&ids])?.iter().map(decode).collect())
}
/// Visit source inputs without collecting an entire chain's records in Rust.
pub(super) fn for_each(
    c: &mut impl GenericClient,
    ids: &[String],
    mut consume: impl FnMut(Input) -> Result<()>,
) -> Result<()> {
    use postgres::fallible_iterator::FallibleIterator;
    let mut rows=c.query_raw("SELECT merchant_id,input_id,input_name,input_website,input_logo_url,input_logo_source,input_markets,input_aliases,input_sources,url,country_hints,dataset_region FROM source_records WHERE merchant_id=ANY($1) ORDER BY source,external_id", [&ids as &(dyn postgres::types::ToSql+Sync)])?;
    while let Some(row) = rows.next()? {
        consume(decode(&row))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_four_backfill_retains_documents_and_projects_typed_inputs() -> Result<()> {
        let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
            .unwrap_or_else(|_| super::super::super::LOCAL_DATABASE_URL.into());
        let store = PostgresStore::temporary(&base)?;
        store.run(|client| {
            let mut tx=client.transaction()?;
            // Reconstruct a populated version-4 source table without changing
            // the caller's catalog; this entire fixture is disposable.
            tx.batch_execute("DROP VIEW source_records_documents; ALTER TABLE source_records ADD COLUMN merchant_json TEXT,ADD COLUMN raw_json TEXT; ALTER TABLE source_records DROP COLUMN input_id,DROP COLUMN input_name,DROP COLUMN input_website,DROP COLUMN input_logo_url,DROP COLUMN input_logo_source,DROP COLUMN input_markets,DROP COLUMN input_aliases,DROP COLUMN input_sources,DROP COLUMN country_hints,DROP COLUMN dataset_region,DROP COLUMN negative_aliases; UPDATE ultrafinance_schema SET version=4")?;
            let merchant: Merchant=serde_json::from_value(serde_json::json!({"id":"input","name":"Café 東京","markets":["CA"],"aliases":["BANK CAFE"]}))?;
            crate::columns::postgres_merchants(&mut tx,"canonical",&serde_json::to_string(&merchant)?)?;
            let raw=serde_json::json!({"countryHints":["CA",null,5,"US"],"payload":"x".repeat(128*1024)});
            let merchant_doc=serde_json::to_string(&merchant)?;let raw_doc=serde_json::to_string(&raw)?;
            tx.execute("INSERT INTO source_records(source,external_id,merchant_id,merchant_json,raw_json,version,attribution,license,url) VALUES('open-enrichment','external','canonical',$1,$2,'fnv1a64:abc:ca:','attribution','license','https://example.org')",&[&merchant_doc,&raw_doc])?;
            tx.batch_execute(include_str!("../../migrations/006_typed_source_inputs.sql"))?;
            let inputs=read(&mut tx,Some(&["canonical".into()]))?;
            assert_eq!(inputs.len(),1);
            assert_eq!(serde_json::to_value(&inputs[0].merchant)?,serde_json::to_value(&merchant)?);
            assert_eq!(inputs[0].hints,vec!["CA","US"]);
            assert_eq!(inputs[0].region.as_deref(),Some("ca"));
            let stored=tx.query_one("SELECT merchant_json,raw_json FROM source_records",&[])?;
            assert_eq!(stored.get::<_,String>(0),merchant_doc);
            assert_eq!(stored.get::<_,String>(1),raw_doc);
            let audit=serde_json::json!({"report":{"kept":true},"before":{"sources":[["canonical",{"merchant":merchant,"raw":raw}]]}}).to_string();
            tx.execute("INSERT INTO merchant_merge_runs(id,data) VALUES('audit',$1)",&[&audit])?;
            tx.batch_execute("CREATE VIEW source_records_documents AS SELECT merchant_json AS data FROM source_records")?;
            tx.batch_execute(include_str!("../../migrations/007_source_inputs_only.sql"))?;
            let doc:String=tx.query_one("SELECT data FROM source_records_documents",&[])?.get(0);
            let compact:SourceRecord=serde_json::from_str(&doc)?;
            assert_eq!(compact.raw,serde_json::json!({"countryHints":["CA","US"]}));
            let audit:String=tx.query_one("SELECT data FROM merchant_merge_runs WHERE id='audit'",&[])?.get(0);
            let audit:serde_json::Value=serde_json::from_str(&audit)?;
            assert_eq!(audit["report"]["kept"],true);
            assert!(audit["before"]["sources"][0][1]["raw"].get("payload").is_none());

            assert_eq!(serde_json::to_value(compact.merchant)?,serde_json::to_value(&merchant)?);
            tx.execute("DELETE FROM source_records",&[])?;
            assert!(tx.query_one("SELECT to_regclass('source_market_inputs') IS NULL", &[])?.get::<_,bool>(0));
            tx.commit()?;
            Ok(())
        })
    }
}
