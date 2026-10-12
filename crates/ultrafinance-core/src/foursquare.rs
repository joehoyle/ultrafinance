//! Merchant and outlet import of a bounded, filtered FSQ OS Places CSV export.
//! Source identities are per place; import reconciliation uses deterministic rules.
use crate::{Merchant, store::SourceRecord};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const URL: &str = "https://docs.foursquare.com/data-products/docs/places-os-data-schema";
pub const CREDIT: &str = "Foursquare OS Places by Foursquare Labs, Inc.";
pub const NOTICE: &str = include_str!("../../../data/foursquare/NOTICE.txt");
pub const LICENSE: &str = include_str!("../../../data/foursquare/LICENSE.txt");

// Foursquare's published non-commercial category exclusions. Unknown categories
// remain eligible; category absence is not evidence that a place is commercial.
const NON_COMMERCIAL: &[&str] = &[
    "4bf58dd8d48988d1f0931735",
    "62d587aeda6648532de2b88c",
    "4bf58dd8d48988d12b951735",
    "52f2ab2ebcbc57f1066b8b3b",
    "50aa9e094b90af0d42d5de0d",
    "5267e4d9e4b0ec79466e48c6",
    "5267e4d9e4b0ec79466e48c9",
    "530e33ccbcbc57f1066bbff7",
    "5345731ebcbc57f1066c39b2",
    "63be6904847c3692a84b9bb7",
    "4d4b7105d754a06373d81259",
    "5267e4d9e4b0ec79466e48c7",
    "4bf58dd8d48988d132951735",
    "52f2ab2ebcbc57f1066b8b4c",
    "50aaa4314b90af0d42d5de10",
    "58daa1558bbb0b01f18ec1fa",
    "63be6904847c3692a84b9bb8",
    "4f2a23984b9023bd5841ed2c",
    "5267e4d9e4b0ec79466e48d1",
    "4f2a25ac4b909258e854f55f",
    "5267e4d9e4b0ec79466e48c8",
    "52741d85e4b0d5d1e3c6a6d9",
    "4bf58dd8d48988d1f7931735",
    "4f4531504b9074f6e4fb0102",
    "4cae28ecbf23941eb1190695",
    "4bf58dd8d48988d1f9931735",
    "5bae9231bedf3950379f89c5",
    "530e33ccbcbc57f1066bbff8",
    "530e33ccbcbc57f1066bbfe4",
    "52f2ab2ebcbc57f1066b8b54",
    "5267e4d8e4b0ec79466e48c5",
    "53e0feef498e5aac066fd8a9",
    "4bf58dd8d48988d130951735",
    "530e33ccbcbc57f1066bbff3",
    "5bae9231bedf3950379f89c3",
    "4bf58dd8d48988d12a951735",
    "52e81612bcbc57f1066b7a24",
    "530e33ccbcbc57f1066bbff9",
];

fn website(value: &str) -> Result<Option<String>> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    let url =
        reqwest::Url::parse(value.trim()).context("website must be an absolute HTTP(S) URL")?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("website must be an HTTP(S) URL without credentials");
    }
    Ok(Some(url.to_string()))
}
fn array(row: &BTreeMap<String, String>, field: &str) -> Result<Vec<String>> {
    let text = row.get(field).map(String::as_str).unwrap_or("");
    if text.trim().is_empty() {
        return Ok(vec![]);
    }
    serde_json::from_str(text).with_context(|| format!("{field} must be a JSON string array"))
}

pub fn prepare(contents: &str, region: &str) -> Result<(Vec<SourceRecord>, usize)> {
    if region != "global" && !crate::markets::valid_country(&region.to_ascii_uppercase()) {
        bail!("Foursquare region must be global or a two-letter country code");
    }
    let mut reader = csv::Reader::from_reader(contents.trim_start_matches('\u{feff}').as_bytes());
    let headers = reader.headers()?.clone();
    for field in [
        "fsq_place_id",
        "name",
        "country",
        "fsq_category_ids",
        "date_closed",
    ] {
        if !headers.iter().any(|h| h == field) {
            bail!("missing Foursquare CSV column {field}");
        }
    }
    let mut groups: BTreeMap<String, Value> = BTreeMap::new();
    let mut seen = BTreeMap::new();
    let mut skipped = 0;
    for record in reader.records() {
        let record = record?;
        let row: BTreeMap<String, String> = headers
            .iter()
            .zip(record.iter())
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        let id = row["fsq_place_id"].trim();
        if id.is_empty() {
            bail!("Foursquare place ID cannot be blank");
        }
        if let Some(old) = seen.insert(id.to_owned(), row.clone()) {
            if old != row {
                bail!("conflicting Foursquare rows for place ID {id}");
            }
            skipped += 1;
            continue;
        }
        let country = row["country"].trim().to_ascii_uppercase();
        if !crate::markets::valid_country(&country) {
            bail!("invalid Foursquare country for place {id}");
        }
        let categories = array(&row, "fsq_category_ids")?;
        let flags = array(&row, "unresolved_flags")?;
        if crate::store::normalize(&row["name"]).is_empty()
            || !row["date_closed"].trim().is_empty()
            || categories.is_empty()
            || categories
                .iter()
                .any(|c| NON_COMMERCIAL.contains(&c.as_str()))
            || flags.iter().any(|f| {
                matches!(
                    f.trim(),
                    "closed"
                        | "duplicate"
                        | "delete"
                        | "privatevenue"
                        | "inappropriate"
                        | "doesnt_exist"
                )
            })
            || (region != "global" && country != region.to_ascii_uppercase())
        {
            skipped += 1;
            continue;
        }
        // Upstream websites are optional, unreviewed metadata. A bad value
        // must not prevent otherwise valid identities from being imported.
        let original_site = row.get("website").map(String::as_str).unwrap_or("");
        let site = website(original_site).unwrap_or(None);
        let mut raw = json!({"fsq_place_id":id,"name":row["name"].trim(),"country":country,
            "website":site,"fsq_category_ids":categories,"unresolved_flags":flags});
        if !original_site.trim().is_empty() && site.is_none() {
            raw["website_original"] = json!(original_site);
            raw["website_validation"] = json!("invalid");
        }
        for field in [
            "locality",
            "region",
            "address",
            "postcode",
            "latitude",
            "longitude",
            "date_created",
            "date_refreshed",
            "date_closed",
            "fsq_category_labels",
        ] {
            if let Some(value) = row.get(field) {
                raw[field] = json!(value);
            }
        }
        groups.insert(format!("place:{id}"), raw);
    }
    let mut records = Vec::new();
    for (key, place) in groups {
        let name = place["name"].as_str().unwrap();
        let site = place["website"].as_str().map(String::from);
        let country = place["country"].as_str().unwrap().to_owned();
        records.push(SourceRecord {
            source: "foursquare".into(),
            external_id: key.clone(),
            merchant: Merchant {
                id: key.clone(),
                name: name.into(),
                markets: vec![country],
                website: site,
                logo_url: None,
                logo_source: None,
                aliases: vec![],
                sources: vec![URL.into()],
            },
            attribution: CREDIT.into(),
            license: "Apache-2.0".into(),
            url: URL.into(),
            version: None,
            raw: json!({"identity_kind":"unlinked-place","places":[place]}),
        });
    }
    Ok((records, skipped))
}

/// Derive independently identified outlets from prepared place evidence. Older
/// cached bundles already contain these fields; catalog source rows do not.
pub(crate) fn locations(record: &SourceRecord) -> Result<Vec<crate::location::LocationRecord>> {
    use crate::location::{LocationData, LocationPrecision, LocationRecord, MerchantReference};
    if record.source != "foursquare" {
        return Ok(vec![]);
    }
    let mut records = Vec::new();
    for place in record.raw["places"].as_array().into_iter().flatten() {
        let text = |field: &str| {
            place[field]
                .as_str()
                .map(str::trim)
                .filter(|v| !v.is_empty() && v.len() <= 512)
                .map(str::to_owned)
        };
        let Some(address) = text("address") else {
            continue;
        };
        let Some(country) = text("country").filter(|c| crate::markets::valid_country(c)) else {
            continue;
        };
        let Some(id) = text("fsq_place_id") else {
            bail!("Foursquare outlet requires a place ID")
        };
        let name = text("name").unwrap_or_else(|| record.merchant.name.clone());
        // A bare brand name is insufficient evidence of this physical outlet.
        let alias = format!("{name} {address}");
        // Malformed optional geometry must not discard usable address evidence.
        let coordinate = |field: &str, max: f64| {
            place[field]
                .as_f64()
                .or_else(|| place[field].as_str()?.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite() && (-max..=max).contains(v))
        };
        let pair = coordinate("latitude", 90.0).zip(coordinate("longitude", 180.0));
        let outlet = LocationRecord {
            provenance: vec![],
            source: record.source.clone(),
            external_id: format!("place:{id}"),
            merchant: MerchantReference::Source {
                source: record.source.clone(),
                external_id: record.external_id.clone(),
            },
            location: LocationData {
                id: None,
                name: Some(name.clone()),
                precision: Some(LocationPrecision::Outlet),
                address: Some(address),
                city: text("locality"),
                region: text("region"),
                postal_code: text("postcode"),
                country: Some(country),
                store_number: None,
                latitude: pair.map(|p| p.0),
                longitude: pair.map(|p| p.1),
            },
            aliases: vec![alias],
            transaction_pattern: None,
            place_ids: BTreeMap::from([("foursquare".into(), id)]),
            manual_override: false,
            attribution: record.attribution.clone(),
            license: record.license.clone(),
            url: record.url.clone(),
        };
        outlet.validate()?;
        records.push(outlet);
    }
    Ok(records)
}

#[cfg(test)]
#[path = "../tests/unit/foursquare/tests.rs"]
mod tests;
