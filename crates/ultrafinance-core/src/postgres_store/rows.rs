//! Decode authoritative columns directly; only variable JSON fields need parsing.
use super::*;
use crate::location::{LocationData, LocationHint, LocationPrecision, MerchantReference};
use postgres::Row;
use serde_json::{Value, json};

fn json_column<T: serde::de::DeserializeOwned>(row: &Row, column: &str) -> Result<T> {
    serde_json::from_str(row.try_get(column)?)
        .with_context(|| format!("invalid JSON in catalog column {column}"))
}

pub(super) fn merchant(row: &Row) -> Result<Merchant> {
    Ok(Merchant {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        website: row.try_get("website")?,
        logo_url: row.try_get("logo_url")?,
        logo_source: row.try_get("logo_source")?,
        markets: row.try_get("markets")?,
        aliases: row.try_get("aliases")?,
        sources: row.try_get("sources")?,
    })
}

pub(crate) fn source(row: &Row) -> Result<SourceRecord> {
    let mut raw = serde_json::Map::new();
    for (column, key) in [
        ("country_hints", "countryHints"),
        ("negative_aliases", "negativeAliases"),
    ] {
        let values: Vec<String> = row.try_get(column)?;
        if !values.is_empty() {
            raw.insert(key.into(), json!(values));
        }
    }
    for (column, key) in [
        ("transaction_pattern", "transaction_text_regexp"),
        ("parent_id", "parent_id"),
    ] {
        if let Some(value) = row.try_get::<_, Option<String>>(column)? {
            raw.insert(key.into(), Value::String(value));
        }
    }
    Ok(SourceRecord {
        source: row.try_get("source")?,
        external_id: row.try_get("external_id")?,
        merchant: Merchant {
            id: row.try_get("input_id")?,
            name: row.try_get("input_name")?,
            website: row.try_get("input_website")?,
            logo_url: row.try_get("input_logo_url")?,
            logo_source: row.try_get("input_logo_source")?,
            markets: row.try_get("input_markets")?,
            aliases: row.try_get("input_aliases")?,
            sources: row.try_get("input_sources")?,
        },
        version: row.try_get("version")?,
        attribution: row.try_get("attribution")?,
        license: row.try_get("license")?,
        url: row.try_get("url")?,
        raw: Value::Object(raw),
    })
}

pub(super) const LOCATIONS: &str = "SELECT l.*,p.attribution,p.license,p.url FROM location_records l JOIN source_provenance p ON p.id=l.source_provenance_id";

pub(super) fn location(row: &Row) -> Result<LocationRecord> {
    let merchant = match row.try_get::<_, Option<String>>("merchant_source")? {
        Some(source) => MerchantReference::Source {
            source,
            external_id: row.try_get("merchant_external_id")?,
        },
        None => MerchantReference::Local {
            merchant_id: row.try_get("merchant_id")?,
        },
    };
    let precision = row
        .try_get::<_, Option<&str>>("precision")?
        .map(|value| match value {
            "country" => Ok(LocationPrecision::Country),
            "region" => Ok(LocationPrecision::Region),
            "city" => Ok(LocationPrecision::City),
            "address" => Ok(LocationPrecision::Address),
            "outlet" => Ok(LocationPrecision::Outlet),
            _ => bail!("invalid catalog location precision"),
        })
        .transpose()?;
    Ok(LocationRecord {
        provenance: vec![],
        source: row.try_get("source")?,
        external_id: row.try_get("external_id")?,
        merchant,
        location: LocationData {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            precision,
            address: row.try_get("address")?,
            city: row.try_get("city")?,
            region: row.try_get("region")?,
            postal_code: row.try_get("postal_code")?,
            country: row.try_get("country")?,
            store_number: row.try_get("store_number")?,
            latitude: row.try_get("latitude")?,
            longitude: row.try_get("longitude")?,
        },
        aliases: row.try_get("aliases")?,
        place_ids: row.try_get::<_, postgres::types::Json<_>>("place_ids")?.0,
        transaction_pattern: row.try_get("transaction_pattern")?,
        manual_override: row.try_get("manual_override")?,
        attribution: row.try_get("attribution")?,
        license: row.try_get("license")?,
        url: row.try_get("url")?,
    })
}

pub(super) const RESOLUTIONS: &str = "SELECT r.id AS resolution_id,r.description,r.country,r.amount,r.currency,r.location_present,r.location_address,r.location_city,r.location_region,r.location_postal_code,r.location_country,r.location_store_number,r.extra_json,r.provenance_json,r.verified,r.review_evidence,m.* FROM descriptor_resolutions r JOIN merchants m ON m.id=r.merchant_id";

pub(super) fn resolution(row: &Row) -> Result<crate::resolution::Resolution> {
    let location = if row.try_get::<_, bool>("location_present")? {
        Some(LocationHint {
            address: row.try_get("location_address")?,
            city: row.try_get("location_city")?,
            region: row.try_get("location_region")?,
            postal_code: row.try_get("location_postal_code")?,
            country: row.try_get("location_country")?,
            store_number: row.try_get("location_store_number")?,
        })
    } else {
        None
    };
    Ok(crate::resolution::Resolution {
        id: row.try_get("resolution_id")?,
        context: json!({
            "description": row.try_get::<_, String>("description")?,
            "country": row.try_get::<_, Option<String>>("country")?,
            "amount": row.try_get::<_, Option<String>>("amount")?,
            "currency": row.try_get::<_, Option<String>>("currency")?,
            "location": location,
            "extra": json_column::<Value>(row, "extra_json")?,
        }),
        merchant: merchant(row)?,
        provenance: json_column(row, "provenance_json")?,
        verified: row.try_get("verified")?,
        evidence: row.try_get("review_evidence")?,
    })
}

pub(super) fn bounded_sources(
    c: &mut impl GenericClient,
    ids: &[String],
) -> Result<Vec<(String, SourceRecord)>> {
    use postgres::fallible_iterator::FallibleIterator;
    let mut rows = c.query_raw(
        "SELECT * FROM source_records WHERE merchant_id=ANY($1) ORDER BY source,external_id",
        [&ids as &(dyn postgres::types::ToSql + Sync)],
    )?;
    let mut bytes = 0;
    let mut sources = Vec::new();
    while let Some(row) = rows.next()? {
        let record = source(&row)?;
        bytes += serde_json::to_vec(&record)?.len();
        if bytes > 64 * 1024 * 1024 {
            bail!(
                "dedupe source evidence exceeds the bounded 64 MiB scan budget; reconcile deterministic identities through source import"
            );
        }
        sources.push((row.try_get("merchant_id")?, record));
    }
    Ok(sources)
}
