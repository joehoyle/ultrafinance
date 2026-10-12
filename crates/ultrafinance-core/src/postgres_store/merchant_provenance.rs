//! Merchant evidence is distinct from upstream identity mappings. Foursquare
//! place mappings remain refreshable, and detailed place evidence lives in
//! location_records; matching needs one source contribution per linked brand.
use super::*;
use serde_json::Value;

pub(super) fn read(
    c: &mut impl GenericClient,
    ids: &[String],
) -> Result<HashMap<String, Vec<SourceRecord>>> {
    let mut sources: HashMap<String, Vec<SourceRecord>> = HashMap::new();
    for row in c.query(include_str!("merchant_provenance.sql"), &[&ids])? {
        let mut record = rows::source(&row)?;
        if record.source == "foursquare" {
            record.raw["source_record_count"] =
                serde_json::json!(row.try_get::<_, i64>("source_record_count")?);
            for values in [
                &mut record.merchant.aliases,
                &mut record.merchant.markets,
                &mut record.merchant.sources,
            ] {
                values.sort();
                values.dedup();
            }
            for key in ["countryHints", "negativeAliases"] {
                if let Some(values) = record.raw[key].as_array_mut() {
                    values.sort_by_key(Value::to_string);
                    values.dedup();
                }
            }
        }
        sources
            .entry(row.try_get("merchant_id")?)
            .or_default()
            .push(record);
    }
    Ok(sources)
}

#[cfg(test)]
#[path = "../../tests/unit/postgres_store/merchant_provenance/tests.rs"]
mod tests;
