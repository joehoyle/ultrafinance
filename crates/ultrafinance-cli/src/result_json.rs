//! CLI result projection. Catalog storage and source administration keep their
//! original serialization; ordinary results omit source bookkeeping.
use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

pub fn value(value: &(impl Serialize + ?Sized)) -> Result<Value> {
    let mut value = serde_json::to_value(value)?;
    remove_metadata(&mut value);
    Ok(value)
}

pub fn pretty(value: &(impl Serialize + ?Sized)) -> Result<String> {
    Ok(serde_json::to_string_pretty(&self::value(value)?)?)
}

pub fn compact(value: &(impl Serialize + ?Sized)) -> Result<String> {
    Ok(serde_json::to_string(&self::value(value)?)?)
}

fn remove_metadata(value: &mut Value) {
    match value {
        Value::Array(values) => values.iter_mut().for_each(remove_metadata),
        Value::Object(fields) => {
            let source_record = fields.contains_key("external_id") && fields.contains_key("source");
            let source_metadata = source_record
                || fields.contains_key("license")
                || fields.contains_key("source_url");
            for field in [
                "provenance",
                "sources",
                "attributions",
                "attribution",
                "license",
                "logo_source",
                "source_url",
                "source_urls",
                "by_source",
                "by_source_region",
                "without_source",
            ] {
                fields.remove(field);
            }
            if source_metadata {
                for field in ["source", "external_id", "url"] {
                    fields.remove(field);
                }
            }
            for (key, child) in fields {
                // User-supplied evidence is an opaque input, not our metadata.
                if key != "extra" {
                    remove_metadata(child);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "../tests/unit/output/result_json_tests.rs"]
mod tests;
