//! Other providers expose retained inputs; Foursquare exposes a marked summary
//! of current merchant/outlet identity and shared attribution, never originals.
use super::*;

pub(super) fn read(
    c: &mut impl GenericClient,
    ids: &[String],
) -> Result<HashMap<String, Vec<SourceRecord>>> {
    let mut sources: HashMap<String, Vec<SourceRecord>> = HashMap::new();
    for row in c.query(include_str!("merchant_provenance.sql"), &[&ids])? {
        let record = rows::source(&row)?;
        sources
            .entry(row.try_get("merchant_id")?)
            .or_default()
            .push(record);
    }
    for row in c.query("SELECT s.merchant_id,min(s.external_id) AS external_id,count(*) AS n,COALESCE(p.attribution,$2) AS attribution,COALESCE(p.license,$3) AS license,COALESCE(p.url,$4) AS url FROM source_identities s LEFT JOIN location_records l ON (l.source,l.external_id)=(s.source,s.external_id) LEFT JOIN source_provenance p ON p.id=l.source_provenance_id WHERE s.source='foursquare' AND s.merchant_id=ANY($1) GROUP BY s.merchant_id,p.attribution,p.license,p.url", &[&ids,&crate::foursquare::CREDIT,&"Apache-2.0",&crate::foursquare::URL])? {
        let id: String = row.get("merchant_id");
        let merchant = rows::merchant(&c.query_one("SELECT * FROM merchants WHERE id=$1", &[&id])?)?;
        sources.entry(id).or_default().push(SourceRecord {
            source:"foursquare".into(), external_id:row.get("external_id"), merchant,
            version:None, attribution:row.get("attribution"), license:row.get("license"),url:row.get("url"),
            raw:serde_json::json!({"identity_only":true,"source_record_count":row.get::<_,i64>("n")}),
        });
    }
    Ok(sources)
}

#[cfg(test)]
#[path = "../../tests/unit/postgres_store/merchant_provenance/tests.rs"]
mod tests;
