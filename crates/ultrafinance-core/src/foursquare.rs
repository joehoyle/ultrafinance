//! Merchant and outlet import of a bounded, filtered FSQ OS Places CSV export.
//! Brand membership is explicitly reviewed, never inferred from name/domain equality.
use crate::{Merchant, store::SourceRecord};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Review {
    brands: Vec<Brand>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Brand {
    id: String,
    name: String,
    website: Option<String>,
    evidence: String,
    place_ids: Vec<String>,
}

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

pub fn prepare(
    contents: &str,
    review: Option<&str>,
    region: &str,
) -> Result<(Vec<SourceRecord>, usize)> {
    if region != "global" && !crate::markets::valid_country(&region.to_ascii_uppercase()) {
        bail!("Foursquare region must be global or a two-letter country code");
    }
    let review: Review = review
        .map(serde_json::from_str)
        .transpose()?
        .unwrap_or(Review { brands: vec![] });
    let mut members = BTreeMap::new();
    let mut brands = BTreeSet::new();
    for (index, brand) in review.brands.iter().enumerate() {
        if brand.id.is_empty()
            || !brand
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            || !brands.insert(&brand.id)
            || crate::store::normalize(&brand.name).is_empty()
            || brand.evidence.trim().is_empty()
            || brand.place_ids.is_empty()
        {
            bail!("brand mappings require unique stable IDs, names, review evidence and place IDs");
        }
        website(brand.website.as_deref().unwrap_or(""))?;
        for id in &brand.place_ids {
            if id.trim().is_empty() || members.insert(id.as_str(), index).is_some() {
                bail!("place IDs must be nonblank and belong to only one reviewed brand");
            }
        }
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
    let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
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
        if (crate::store::normalize(&row["name"]).is_empty() && !members.contains_key(id))
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
        let key = members
            .get(id)
            .map(|i| format!("brand:{}", review.brands[*i].id))
            .unwrap_or_else(|| format!("place:{id}"));
        groups.entry(key).or_default().push(raw);
    }
    let mut records = Vec::new();
    for (key, mut places) in groups {
        places.sort_by(|a, b| a["fsq_place_id"].as_str().cmp(&b["fsq_place_id"].as_str()));
        let brand = key
            .strip_prefix("brand:")
            .and_then(|id| review.brands.iter().find(|b| b.id == id));
        let name = brand
            .map(|b| b.name.trim())
            .unwrap_or_else(|| places[0]["name"].as_str().unwrap());
        let site = if let Some(brand) = brand {
            website(brand.website.as_deref().unwrap_or(""))?
        } else {
            places[0]["website"].as_str().map(String::from)
        };
        let countries: BTreeSet<_> = places
            .iter()
            .map(|p| p["country"].as_str().unwrap().to_owned())
            .collect();
        records.push(SourceRecord {
            source: "foursquare".into(),
            external_id: key.clone(),
            merchant: Merchant {
                id: key.clone(),
                name: name.into(),
                markets: countries.into_iter().collect(),
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
            raw: json!({"identity_kind":if brand.is_some(){"reviewed-brand"}else{"unlinked-place"},
                "review_evidence":brand.map(|b| &b.evidence),"places":places}),
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
mod tests {
    use super::*;
    fn csv(rows: &[(&str, &str, &str, &str, &str)]) -> String {
        let mut w = csv::Writer::from_writer(vec![]);
        w.write_record([
            "fsq_place_id",
            "name",
            "country",
            "website",
            "date_closed",
            "fsq_category_ids",
        ])
        .unwrap();
        for (id, name, country, site, closed) in rows {
            w.write_record([*id, *name, *country, *site, *closed, "[\"restaurant\"]"])
                .unwrap();
        }
        String::from_utf8(w.into_inner().unwrap()).unwrap()
    }
    #[test]
    fn outlet_fields_require_addresses_and_optional_coordinates_are_paired() -> Result<()> {
        let input = "fsq_place_id,name,country,address,latitude,longitude,fsq_category_ids,date_closed\na,H&M,CA,1 Main St,43.6,-79.3,\"[\"\"restaurant\"\"]\",\nb,Cafe,CA,,43.6,-79.3,\"[\"\"restaurant\"\"]\",\nc,Cafe,CA,3 Main St,NaN,-79.3,\"[\"\"restaurant\"\"]\",\nd,Cafe,CA,4 Main St,43.6,181,\"[\"\"restaurant\"\"]\",\n";
        let (records, _) = prepare(input, None, "ca")?;
        let outlet = locations(&records[0])?.remove(0);
        assert_eq!(outlet.location.name.as_deref(), Some("H&M"));
        assert_eq!(outlet.location.latitude, Some(43.6));
        assert_eq!(outlet.aliases, vec!["H&M 1 Main St"]);
        assert!(locations(&records[1])?.is_empty());
        for record in &records[2..] {
            let outlet = locations(record)?.remove(0);
            assert!(outlet.location.latitude.is_none());
            assert!(outlet.location.longitude.is_none());
        }
        Ok(())
    }
    #[test]
    fn reviewed_brands_group_places_but_name_and_domain_do_not() {
        let input = csv(&[
            ("a", "Starbucks Bromont", "CA", "https://starbucks.com", ""),
            ("b", "Starbucks Toronto", "CA", "https://starbucks.com", ""),
            ("c", "Central Cafe", "CA", "https://example.com", ""),
            ("d", "Central Cafe", "CA", "https://example.com", ""),
        ]);
        let review = r#"{"brands":[{"id":"starbucks","name":"Starbucks","website":"https://starbucks.com","evidence":"Reviewed official outlet directory","place_ids":["a","b"]}]}"#;
        let (records, _) = prepare(&input, Some(review), "ca").unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].merchant.name, "Starbucks");
        assert_eq!(records[0].raw["places"].as_array().unwrap().len(), 2);
        assert!(records.iter().all(|r| r.merchant.aliases.is_empty()));
        let reversed = csv(&[
            ("d", "Central Cafe", "CA", "https://example.com", ""),
            ("b", "Starbucks Toronto", "CA", "https://starbucks.com", ""),
            ("a", "Starbucks Bromont", "CA", "https://starbucks.com", ""),
            ("c", "Central Cafe", "CA", "https://example.com", ""),
        ]);
        assert_eq!(
            serde_json::to_value(records).unwrap(),
            serde_json::to_value(prepare(&reversed, Some(review), "ca").unwrap().0).unwrap()
        );
    }
    #[test]
    fn invalid_identity_and_duplicate_membership_fail() {
        let input = csv(&[("a", "A", "CA", "", ""), ("a", "B", "CA", "", "")]);
        assert!(prepare(&input, None, "global").is_err());
        let input = csv(&[("a", "A", "CA", "", "")]);
        let review =
            r#"{"brands":[{"id":"a","name":"A","evidence":"checked","place_ids":["a","a"]}]}"#;
        assert!(prepare(&input, Some(review), "global").is_err());
        assert!(prepare(&input, None, "../").is_err());
    }
    #[test]
    fn invalid_upstream_websites_preserve_merchants_and_original_evidence() {
        let input = csv(&[
            ("a", "A", "CA", "ftp://example.com", ""),
            ("b", "B", "CA", "https://user:password@example.com", ""),
            ("c", "C", "CA", "example.com", ""),
            ("d", "D", "CA", "https://example.com", ""),
        ]);
        let (records, skipped) = prepare(&input, None, "ca").unwrap();
        assert_eq!(records.len(), 4);
        assert_eq!(skipped, 0);
        for record in &records[..3] {
            assert!(record.merchant.website.is_none());
            assert_eq!(record.raw["places"][0]["website_validation"], "invalid");
            assert!(record.raw["places"][0]["website_original"].is_string());
        }
        assert_eq!(
            records[3].merchant.website.as_deref(),
            Some("https://example.com/")
        );
        // Explicitly reviewed brand metadata remains strict.
        let review = r#"{"brands":[{"id":"a","name":"A","website":"ftp://example.com","evidence":"checked","place_ids":["a"]}]}"#;
        assert!(prepare(&input, Some(review), "ca").is_err());
    }
    #[test]
    fn closure_country_and_noncommercial_filters_do_not_create_merchants() {
        let input = csv(&[
            ("a", "A", "CA", "", "2020-01-01"),
            ("b", "B", "US", "", ""),
            ("c", "C", "CA", "", ""),
        ]);
        let (records, skipped) = prepare(&input, None, "ca").unwrap();
        assert_eq!(skipped, 2);
        assert_eq!(records.len(), 1);
        let input = input.replace("restaurant", "530e33ccbcbc57f1066bbff7");
        assert!(prepare(&input, None, "global").unwrap().0.is_empty());
    }
    #[test]
    fn unmatchable_names_are_skipped_but_reviewed_brand_names_can_supply_identity() {
        let input = csv(&[
            ("a", "---", "CA", "", ""),
            ("b", "☕", "CA", "", ""),
            ("c", "Café", "CA", "", ""),
            ("d", "東京", "CA", "", ""),
        ]);
        let (records, skipped) = prepare(&input, None, "ca").unwrap();
        assert_eq!(skipped, 2);
        assert_eq!(records.len(), 2);
        for record in &records {
            crate::store::validate(&record.merchant).unwrap();
        }
        let review = r#"{"brands":[{"id":"coffee","name":"Coffee","evidence":"checked","place_ids":["b"]}]}"#;
        let (records, skipped) = prepare(&input, Some(review), "ca").unwrap();
        assert_eq!(skipped, 1);
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].merchant.name, "Coffee");
        let invalid_review = review.replace("\"Coffee\"", "\"☕\"");
        assert!(prepare(&input, Some(&invalid_review), "ca").is_err());
    }
}
