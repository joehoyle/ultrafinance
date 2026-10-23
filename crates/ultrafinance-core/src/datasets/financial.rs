//! Public financial-institution identities. These sources never supply transaction labels.
use super::Source;
use crate::{Counterparty, CounterpartyType, store::SourceRecord};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const FDIC_URL: &str = "https://api.fdic.gov/banks/docs";
pub const NCUA_URL: &str = "https://ncua.gov/analysis/credit-union-corporate-call-report-data";
pub const GLEIF_URL: &str = "https://www.gleif.org/en/lei-data/gleif-api";
pub const PAYMENTS_URL: &str = "https://en.wikipedia.org/wiki/Category:Payment_service_providers";
// These are semantic classes, not provider-specific exceptions. Category
// membership still restricts selection to payment-service-provider pages.
pub const PAYMENT_PROVIDER_CLASSES: &[&str] = &[
    "Q43229",    // organization
    "Q986008",   // payment system
    "Q431289",   // brand
    "Q7406919",  // service
    "Q15809678", // financial product
    "Q35127",    // website / online platform within the selected provider category
];

fn provider_kind(row: &Value) -> Option<&str> {
    let uri = row["providerKind"]["value"]
        .as_str()
        .or_else(|| row["organization"]["value"].as_str())?;
    let id = uri.strip_prefix("http://www.wikidata.org/entity/")?;
    PAYMENT_PROVIDER_CLASSES.contains(&id).then_some(uri)
}

pub const WIKIDATA_URL: &str = "https://www.wikidata.org/wiki/Q650241";
pub const WIKIDATA_QUERY: &str = r#"SELECT DISTINCT ?item ?name ?country ?website ?lei ?alias ?financialInstitution WHERE {
  ?item wdt:P31/wdt:P279* wd:Q650241; rdfs:label ?name.
  FILTER(LANG(?name)="en") FILTER NOT EXISTS { ?item wdt:P576 ?end }
  BIND(wd:Q650241 AS ?financialInstitution)
  OPTIONAL { ?item wdt:P17/wdt:P297 ?country }
  OPTIONAL { ?item wdt:P856 ?website }
  OPTIONAL { ?item wdt:P1278 ?lei }
  OPTIONAL { ?item skos:altLabel ?alias FILTER(LANG(?alias)="en") }
} ORDER BY ?item ?name ?country ?website ?lei ?alias"#;

pub fn supported(source: Source) -> bool {
    matches!(
        source,
        Source::Fdic | Source::Ncua | Source::Gleif | Source::Wikidata | Source::WikidataPayments
    )
}
pub fn metadata(source: Source) -> (&'static str, &'static str, &'static str) {
    match source {
        Source::Fdic => (
            "US government public data",
            "FDIC BankFind Suite: active institutions",
            FDIC_URL,
        ),
        Source::Ncua => (
            "US government public data",
            "NCUA: active federally insured credit unions",
            NCUA_URL,
        ),
        Source::Gleif => (
            "CC0-1.0",
            "GLEIF LEI data; financial-institution selection from Wikidata (CC0)",
            GLEIF_URL,
        ),
        Source::WikidataPayments => (
            "CC0-1.0 AND CC-BY-SA-4.0",
            "Wikidata contributors; English Wikipedia payment-service-provider category selects organizations, payment systems, services, products, brands and online platforms",
            PAYMENTS_URL,
        ),
        Source::Wikidata => (
            "CC0-1.0",
            "Wikidata contributors: financial institutions",
            WIKIDATA_URL,
        ),
        _ => unreachable!(),
    }
}
pub fn validate_region(source: Source, region: &str) -> Result<()> {
    if matches!(source, Source::Fdic | Source::Ncua) {
        ensure!(
            matches!(region, "global" | "us"),
            "{} supports only global or us",
            source.name()
        );
    } else {
        ensure!(
            region == "global"
                || (region.len() == 2 && region.bytes().all(|b| b.is_ascii_lowercase())),
            "region must be global or a lowercase two-letter country code"
        );
    }
    Ok(())
}
fn required<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .with_context(|| format!("missing {key}"))
}
fn record(
    source: Source,
    id: String,
    name: &str,
    countries: Vec<String>,
    raw: Value,
) -> SourceRecord {
    let (license, credit, url) = metadata(source);
    SourceRecord {
        source: source.name().into(),
        external_id: id.clone(),
        merchant: Counterparty {
            categories: None,
            category_ids: vec![],
            id: format!("{}:{id}", source.name()),
            name: name.into(),
            counterparty_type: CounterpartyType::FinancialInstitution,
            markets: countries,
            aliases: vec![],
            website: None,
            logo_url: None,
            logo_source: None,
            sources: vec![url.into()],
        },
        attribution: credit.into(),
        license: license.into(),
        url: url.into(),
        version: None,
        raw,
    }
}
fn country(value: &str) -> Result<String> {
    ensure!(
        value.len() == 2 && value.bytes().all(|b| b.is_ascii_uppercase()),
        "invalid country code: {value}"
    );
    Ok(value.into())
}
pub fn valid_lei(value: &str) -> bool {
    value.len() == 20
        && value
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}
fn website(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let url = if value.starts_with("http://") || value.starts_with("https://") {
        value.to_owned()
    } else {
        format!("https://{value}")
    };
    // Reject non-web schemes, credentials, whitespace, and placeholder values.
    let host = url.split_once("://")?.1.split('/').next()?;
    (host.contains('.') && !host.contains(['@', ':']) && !url.chars().any(char::is_whitespace))
        .then_some(url)
}
fn numeric_id(value: &str) -> Result<String> {
    ensure!(
        !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()),
        "invalid institution ID"
    );
    let id: u64 = value.parse()?;
    ensure!(id > 0, "institution ID must be positive");
    Ok(id.to_string())
}

pub fn prepare(source: Source, contents: &str, region: &str) -> Result<Vec<SourceRecord>> {
    validate_region(source, region)?;
    let mut records = match source {
        Source::Ncua => ncua(contents)?,
        Source::Fdic => fdic(&serde_json::from_str(contents)?)?,
        Source::WikidataPayments => wikidata_payments(&serde_json::from_str(contents)?)?,
        Source::Wikidata => wikidata(&serde_json::from_str(contents)?)?,
        Source::Gleif => gleif(&serde_json::from_str(contents)?)?,
        _ => bail!("unsupported institution source"),
    };
    ensure!(
        !records.is_empty(),
        "source contains no eligible financial institutions"
    );
    records.retain(|r| {
        region == "global"
            || r.merchant
                .markets
                .iter()
                .any(|c| c.eq_ignore_ascii_case(region))
    });
    records.sort_by(|a, b| a.external_id.cmp(&b.external_id));
    let mut ids = BTreeSet::new();
    for record in &mut records {
        ensure!(
            ids.insert(record.external_id.clone()),
            "duplicate source identity {}",
            record.external_id
        );
        record.merchant.aliases.sort();
        record.merchant.aliases.dedup();
        record
            .merchant
            .aliases
            .retain(|a| !a.eq_ignore_ascii_case(&record.merchant.name));
    }
    Ok(records)
}
fn fdic(data: &Value) -> Result<Vec<SourceRecord>> {
    let rows = data["data"].as_array().context("FDIC data array missing")?;
    ensure!(
        data["meta"]["total"].as_u64() == Some(rows.len() as u64),
        "incomplete FDIC export: total does not match rows"
    );
    rows.iter()
        .filter_map(|row| {
            let row = &row["data"];
            if row["ACTIVE"].as_u64() == Some(0) {
                return None;
            }
            Some((|| {
                ensure!(
                    row["ACTIVE"].as_u64() == Some(1),
                    "invalid FDIC ACTIVE flag"
                );
                let id = numeric_id(
                    &row["CERT"]
                        .as_u64()
                        .context("FDIC CERT missing")?
                        .to_string(),
                )?;
                let mut r = record(
                    Source::Fdic,
                    id,
                    required(row, "NAME")?,
                    vec!["US".into()],
                    row.clone(),
                );
                r.merchant.website = row["WEBADDR"].as_str().and_then(website);
                Ok(r)
            })())
        })
        .collect()
}
fn ncua(contents: &str) -> Result<Vec<SourceRecord>> {
    let mut csv = csv::Reader::from_reader(contents.trim_start_matches('\u{feff}').as_bytes());
    let headers = csv.headers()?.clone();
    let cols = [
        "Charter number",
        "Credit Union name",
        "Credit Union type",
        "Year and quarter",
    ]
    .map(|name| {
        let indices: Vec<_> = headers
            .iter()
            .enumerate()
            .filter(|(_, h)| *h == name)
            .map(|(i, _)| i)
            .collect();
        ensure!(
            indices.len() == 1,
            "NCUA CSV requires exactly one {name} column"
        );
        Ok(indices[0])
    })
    .into_iter()
    .collect::<Result<Vec<_>>>()?;
    csv.records()
        .map(|row| {
            let row = row?;
            let fields: Vec<_> = cols.iter().map(|i| row[*i].trim()).collect();
            ensure!(
                !fields[1].is_empty() && !fields[3].is_empty(),
                "NCUA name/reporting period missing"
            );
            ensure!(
                matches!(fields[2], "FCU" | "FISCU"),
                "unexpected NCUA institution type"
            );
            Ok(record(
                Source::Ncua,
                numeric_id(fields[0])?,
                fields[1],
                vec!["US".into()],
                json!({"charter":fields[0],"credit_union_type":fields[2],"period":fields[3]}),
            ))
        })
        .collect()
}
pub fn wikidata(data: &Value) -> Result<Vec<SourceRecord>> {
    wikidata_records(data, false)
}
pub fn wikidata_payments(data: &Value) -> Result<Vec<SourceRecord>> {
    Ok(payment_provider_selection(data)?.0)
}

/// Return imported identities and an explicit, reproducible exclusion report.
/// Version 2 snapshots retain one or more query rows for every mapped QID.
pub fn payment_provider_selection(data: &Value) -> Result<(Vec<SourceRecord>, Vec<Value>)> {
    let category = data["category"]["query"]["categorymembers"]
        .as_array()
        .context("payment-provider category evidence missing")?;
    ensure!(
        data["category"]["continue"].is_null(),
        "incomplete category selection"
    );
    let titles: BTreeSet<_> = category
        .iter()
        .filter_map(|r| r["title"].as_str())
        .collect();
    let members = data["members"]
        .as_array()
        .context("Wikidata member mappings missing")?;
    let mut ids = BTreeSet::new();
    for member in members {
        ensure!(
            titles.contains(required(member, "title")?),
            "member not present in category selection"
        );
        ids.insert(required(member, "qid")?);
    }
    let rows = data["results"]["bindings"]
        .as_array()
        .context("Wikidata SPARQL bindings missing")?;
    let version = data["selection_version"].as_u64().unwrap_or(1);
    ensure!(
        matches!(version, 1 | 2),
        "unsupported payment-provider selection version"
    );
    let mut grouped = BTreeMap::<String, Vec<Value>>::new();
    for row in rows {
        let qid = row["item"]["value"]
            .as_str()
            .and_then(|uri| uri.strip_prefix("http://www.wikidata.org/entity/"))
            .context("invalid Wikidata URI")?;
        ensure!(
            ids.contains(qid),
            "payment provider missing category membership evidence"
        );
        ensure!(
            row["paymentProviderCategory"]["value"] == PAYMENTS_URL,
            "payment-provider category evidence missing"
        );
        grouped.entry(qid.into()).or_default().push(row.clone());
    }
    if version == 2 {
        ensure!(
            ids.iter().all(|id| grouped.contains_key(*id)),
            "incomplete payment-provider query results"
        );
    }
    let mut accepted = Vec::new();
    let mut reasons = BTreeMap::new();
    for (qid, rows) in grouped {
        let reason = if version == 1 {
            None // Preserve compatibility with organization-only cached snapshots.
        } else if rows
            .iter()
            .any(|r| r["dissolved"]["value"].as_str().is_some())
        {
            Some("dissolved")
        } else if !rows.iter().any(|r| {
            r["name"]["value"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty())
        }) {
            Some("missing_english_or_default_label")
        } else if !rows.iter().any(|r| provider_kind(r).is_some()) {
            Some("unsupported_classification")
        } else {
            None
        };
        if let Some(reason) = reason {
            let classes: BTreeSet<_> = rows
                .iter()
                .filter_map(|r| r["instanceOf"]["value"].as_str())
                .collect();
            reasons.insert(qid, json!({"reason":reason,"instance_of":classes}));
        } else {
            accepted.extend(
                rows.into_iter()
                    .filter(|r| version == 1 || provider_kind(r).is_some()),
            );
        }
    }
    let mut filtered = data.clone();
    filtered["results"]["bindings"] = json!(accepted);
    let records = wikidata_records(&filtered, true)?;
    let imported: BTreeSet<_> = records.iter().map(|r| r.external_id.as_str()).collect();
    let mut excluded = Vec::new();
    for page in category {
        let title = required(page, "title")?;
        let qid = members
            .iter()
            .find(|m| m["title"] == title)
            .and_then(|m| m["qid"].as_str());
        if qid.is_some_and(|id| imported.contains(id)) {
            continue;
        }
        let mut entry = qid.and_then(|id| reasons.get(id)).cloned().unwrap_or_else(|| json!({
            "reason":if qid.is_none() { "missing_wikidata_mapping" } else { "legacy_selection_omission" }
        }));
        entry["title"] = json!(title);
        entry["qid"] = json!(qid);
        excluded.push(entry);
    }
    Ok((records, excluded))
}
fn wikidata_records(data: &Value, payments: bool) -> Result<Vec<SourceRecord>> {
    let rows = data["results"]["bindings"]
        .as_array()
        .context("Wikidata SPARQL bindings missing")?;
    let mut records = BTreeMap::<String, SourceRecord>::new();
    let mut sites = BTreeMap::<String, BTreeSet<String>>::new();
    for row in rows {
        let get = |key: &str| row[key]["value"].as_str().unwrap_or("").trim();
        if payments {
            ensure!(
                get("paymentProviderCategory") == PAYMENTS_URL && provider_kind(row).is_some(),
                "payment-provider category/classification evidence missing"
            );
        } else {
            ensure!(
                get("financialInstitution") == "http://www.wikidata.org/entity/Q650241",
                "Wikidata financial-institution classification missing"
            );
        }
        let id = get("item")
            .strip_prefix("http://www.wikidata.org/entity/")
            .context("invalid Wikidata URI")?;
        ensure!(
            id.starts_with('Q') && id.len() > 1 && id[1..].bytes().all(|b| b.is_ascii_digit()),
            "invalid Wikidata ID"
        );
        let name = get("name");
        ensure!(!name.is_empty(), "Wikidata name missing");
        let r = records.entry(id.into()).or_insert_with(|| {
            record(
                if payments {
                    Source::WikidataPayments
                } else {
                    Source::Wikidata
                },
                id.into(),
                name,
                vec![],
                json!({"leis":[],"classification":if payments { PAYMENTS_URL } else { "Q650241" }}),
            )
        });
        if payments {
            r.merchant.counterparty_type = CounterpartyType::PaymentProcessor;
            let kinds = r
                .raw
                .as_object_mut()
                .unwrap()
                .entry("provider_kinds")
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .unwrap();
            let kind = json!(provider_kind(row).unwrap());
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
        ensure!(
            r.merchant.name == name,
            "conflicting Wikidata names for {id}"
        );
        if !get("country").is_empty() {
            r.merchant.markets.push(country(get("country"))?);
        }
        if !get("alias").is_empty() {
            r.merchant.aliases.push(get("alias").into());
        }
        if let Some(site) = website(get("website")) {
            sites.entry(id.into()).or_default().insert(site);
        }
        if valid_lei(get("lei")) {
            r.raw["leis"]
                .as_array_mut()
                .unwrap()
                .push(json!(get("lei")));
        }
    }
    for (id, r) in &mut records {
        if let Some(kinds) = r
            .raw
            .get_mut("provider_kinds")
            .and_then(Value::as_array_mut)
        {
            kinds.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
        }
        r.merchant.markets.sort();
        r.merchant.markets.dedup();
        r.merchant.aliases.sort();
        r.merchant.aliases.dedup();
        if let Some(sites) = sites.get(id).filter(|s| s.len() == 1) {
            r.merchant.website = sites.first().cloned();
        }
        let mut leis: Vec<String> = serde_json::from_value(r.raw["leis"].clone())?;
        leis.sort();
        leis.dedup();
        r.raw["leis"] = json!(leis);
    }
    Ok(records.into_values().collect())
}
fn gleif(data: &Value) -> Result<Vec<SourceRecord>> {
    // Preserve the selection evidence in the snapshot. An arbitrary LEI record
    // cannot establish that the entity is a financial institution.
    let selection = wikidata(&data["selection"])?;
    let mut links = BTreeMap::<String, Vec<String>>::new();
    for r in selection {
        for lei in r.raw["leis"].as_array().unwrap() {
            links
                .entry(lei.as_str().unwrap().into())
                .or_default()
                .push(r.external_id.clone());
        }
    }
    let rows = data["data"].as_array().context("GLEIF data missing")?;
    ensure!(
        data["complete"].as_bool() == Some(true),
        "incomplete GLEIF snapshot"
    );
    let mut seen = BTreeSet::new();
    let mut records = vec![];
    for row in rows {
        let a = &row["attributes"];
        let lei = required(a, "lei")?;
        ensure!(
            valid_lei(lei) && row["id"].as_str() == Some(lei),
            "invalid GLEIF identity"
        );
        let qids = links
            .get(lei)
            .context("GLEIF entity lacks financial-institution selection evidence")?;
        ensure!(seen.insert(lei.to_owned()), "duplicate GLEIF LEI");
        // Annulled/duplicate registrations can have the literal entity status
        // NULL. They are returned for completeness but are not usable identities.
        if matches!(
            a["registration"]["status"].as_str(),
            Some("ANNULLED" | "DUPLICATE")
        ) {
            continue;
        }
        let entity = &a["entity"];
        match required(entity, "status")? {
            "INACTIVE" => continue,
            "ACTIVE" => (),
            _ => bail!("unknown GLEIF entity status"),
        }
        let c = country(required(&entity["legalAddress"], "country")?)?;
        let mut r = record(
            Source::Gleif,
            lei.into(),
            required(&entity["legalName"], "name")?,
            vec![c],
            json!({"lei":lei,"wikidata":qids}),
        );
        r.merchant.sources.push(WIKIDATA_URL.into());
        for other in entity["otherNames"].as_array().into_iter().flatten() {
            if matches!(
                other["type"].as_str(),
                Some("ALTERNATIVE_LANGUAGE_LEGAL_NAME" | "TRADING_OR_OPERATING_NAME")
            ) {
                r.merchant.aliases.push(required(other, "name")?.into());
            }
        }
        records.push(r);
    }
    ensure!(
        seen.len() == links.len(),
        "incomplete GLEIF snapshot: selected LEIs missing"
    );
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_regulator_ids_and_strict_completeness() {
        let source = json!({"meta":{"total":2},"data":[{"data":{"CERT":12,"NAME":"Example Bank","ACTIVE":1,"WEBADDR":"www.example.com"}},{"data":{"CERT":13,"NAME":"Closed Bank","ACTIVE":0}}]});
        let records = prepare(Source::Fdic, &source.to_string(), "us").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].external_id, "12");
        assert_eq!(
            records[0].merchant.website.as_deref(),
            Some("https://www.example.com")
        );
        let mut changed = source.clone();
        changed["data"][0]["data"]["NAME"] = json!("Renamed Bank");
        assert_eq!(
            prepare(Source::Fdic, &changed.to_string(), "us").unwrap()[0].external_id,
            records[0].external_id
        );
        changed["meta"]["total"] = json!(3);
        assert!(prepare(Source::Fdic, &changed.to_string(), "global").is_err());
        let csv = "Charter number,Credit Union name,Credit Union type,Year and quarter\n001,Example Credit Union,FCU,2026.2\n";
        let records = prepare(Source::Ncua, csv, "us").unwrap();
        assert_eq!(records[0].external_id, "1");
        assert!(prepare(Source::Ncua, &csv.replace("FCU", "PRIVATE"), "us").is_err());
        assert!(prepare(Source::Ncua, csv, "ca").is_err());
        assert!(prepare(Source::Ncua, &format!("{csv}1,Other,FISCU,2026.2\n"), "us").is_err());
    }
    fn selection() -> Value {
        json!({"results":{"bindings":[{"item":{"value":"http://www.wikidata.org/entity/Q123"},"name":{"value":"Example Bank"},"country":{"value":"US"},"lei":{"value":"12345678901234567890"},"financialInstitution":{"value":"http://www.wikidata.org/entity/Q650241"},"alias":{"value":"Example Banking"},"website":{"value":"https://example.com"}}]}})
    }
    #[test]
    fn wikidata_combines_rows_without_ambiguous_websites_or_duplicate_aliases() {
        let mut malformed = selection();
        malformed["results"]["bindings"][0]["lei"]["value"] =
            json!("https://example.com/not-a-lei");
        let kept = prepare(Source::Wikidata, &malformed.to_string(), "global").unwrap();
        assert_eq!(kept.len(), 1);
        assert!(kept[0].raw["leis"].as_array().unwrap().is_empty());
        let mut data = selection();
        let mut second = data["results"]["bindings"][0].clone();
        second["website"]["value"] = json!("https://different.example");
        data["results"]["bindings"]
            .as_array_mut()
            .unwrap()
            .push(second);
        let records = prepare(Source::Wikidata, &data.to_string(), "us").unwrap();
        assert_eq!(records.len(), 1);
        assert!(records[0].merchant.website.is_none());
        assert_eq!(records[0].merchant.aliases, ["Example Banking"]);
        assert_eq!(records[0].raw["leis"].as_array().unwrap().len(), 1);
        assert!(
            prepare(Source::Wikidata, &data.to_string(), "ca")
                .unwrap()
                .is_empty()
        );
        data["results"]["bindings"][0]["financialInstitution"] = Value::Null;
        assert!(prepare(Source::Wikidata, &data.to_string(), "global").is_err());
    }
    #[test]
    fn gleif_requires_exact_selection_links_and_complete_responses() {
        let mut data = json!({"selection":selection(),"complete":true,"data":[{"id":"12345678901234567890","attributes":{"lei":"12345678901234567890","entity":{"legalName":{"name":"Example Legal Bank"},"status":"ACTIVE","legalAddress":{"country":"US"},"otherNames":[{"name":"Example Trading","type":"TRADING_OR_OPERATING_NAME"},{"name":"Former Bank","type":"PREVIOUS_LEGAL_NAME"}]}}}]});
        let records = prepare(Source::Gleif, &data.to_string(), "global").unwrap();
        assert_eq!(records[0].merchant.aliases, ["Example Trading"]);
        assert_eq!(
            records[0].merchant.counterparty_type,
            CounterpartyType::FinancialInstitution
        );
        let mut duplicate = data.clone();
        duplicate["data"][0]["attributes"]["registration"] = json!({"status":"DUPLICATE"});
        duplicate["data"][0]["attributes"]["entity"]["status"] = json!("NULL");
        assert!(gleif(&duplicate).unwrap().is_empty());
        duplicate["data"][0]["attributes"]["registration"] = json!({"status":"ISSUED"});
        assert!(gleif(&duplicate).is_err());
        data["complete"] = json!(false);
        assert!(prepare(Source::Gleif, &data.to_string(), "global").is_err());
        data["complete"] = json!(true);
        data["data"] = json!([]);
        assert!(prepare(Source::Gleif, &data.to_string(), "global").is_err());
        assert!(website("javascript:alert(1)").is_none());
    }
    #[test]
    fn payment_selection_includes_systems_brands_and_services_and_reports_exclusions() {
        let mut data: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/wikidata-payments.json"))
                .unwrap();
        data["selection_version"] = json!(2);
        let original = data["results"]["bindings"][0].clone();
        let mut rows = Vec::new();
        let mut pages = Vec::new();
        let mut members = Vec::new();
        for (index, kind) in PAYMENT_PROVIDER_CLASSES.iter().enumerate() {
            let qid = format!("Q{}", index + 100);
            let title = format!("Provider {index}");
            let mut row = original.clone();
            row.as_object_mut().unwrap().remove("organization");
            row["item"]["value"] = json!(format!("http://www.wikidata.org/entity/{qid}"));
            row["name"]["value"] = json!(title);
            row["providerKind"] = json!({"value":format!("http://www.wikidata.org/entity/{kind}")});
            rows.push(row);
            pages.push(json!({"title":title,"pageid":index+1,"ns":0}));
            members.push(json!({"title":title,"qid":qid}));
        }
        for (index, reason) in [
            "unsupported_classification",
            "missing_english_or_default_label",
            "dissolved",
        ]
        .iter()
        .enumerate()
        {
            let qid = format!("Q{}", index + 200);
            let mut row = rows[0].clone();
            row["item"]["value"] = json!(format!("http://www.wikidata.org/entity/{qid}"));
            match *reason {
                "unsupported_classification" => {
                    row["providerKind"] = Value::Null;
                    row["instanceOf"] = json!({"value":"http://www.wikidata.org/entity/Q13406463"});
                }
                "missing_english_or_default_label" => row["name"] = Value::Null,
                "dissolved" => row["dissolved"] = json!({"value":"2020-01-01T00:00:00Z"}),
                _ => unreachable!(),
            }
            rows.push(row);
            pages.push(json!({"title":reason,"pageid":index+100,"ns":0}));
            members.push(json!({"title":reason,"qid":qid}));
        }
        pages.push(json!({"title":"Unmapped page","pageid":999,"ns":0}));
        data["results"]["bindings"] = json!(rows);
        data["members"] = json!(members);
        data["category"]["query"]["categorymembers"] = json!(pages);
        let (records, excluded) = payment_provider_selection(&data).unwrap();
        assert_eq!(records.len(), PAYMENT_PROVIDER_CLASSES.len());
        assert!(
            records
                .iter()
                .all(|r| r.merchant.counterparty_type == CounterpartyType::PaymentProcessor)
        );
        assert_eq!(
            excluded
                .iter()
                .map(|e| e["reason"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![
                "unsupported_classification",
                "missing_english_or_default_label",
                "dissolved",
                "missing_wikidata_mapping"
            ]
        );
        let mut incomplete = data.clone();
        incomplete["results"]["bindings"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(payment_provider_selection(&incomplete).is_err());
        data["results"]["bindings"][0]["item"]["value"] =
            json!("http://www.wikidata.org/entity/Q9999999");
        assert!(payment_provider_selection(&data).is_err());
    }

    #[test]
    fn payment_category_import_requires_membership_and_organization_evidence() {
        let contents = include_str!("../../tests/fixtures/wikidata-payments.json");
        let records = prepare(Source::WikidataPayments, contents, "global").unwrap();
        assert_eq!(
            records[0].merchant.counterparty_type,
            CounterpartyType::PaymentProcessor
        );
        let mut data: Value = serde_json::from_str(contents).unwrap();
        data["members"][0]["title"] = json!("Unlisted provider");
        assert!(prepare(Source::WikidataPayments, &data.to_string(), "global").is_err());
        let mut data: Value = serde_json::from_str(contents).unwrap();
        data["results"]["bindings"][0]["organization"] = Value::Null;
        assert!(prepare(Source::WikidataPayments, &data.to_string(), "global").is_err());
        let mut data: Value = serde_json::from_str(contents).unwrap();
        data["category"]["continue"] = json!({"cmcontinue":"more"});
        assert!(prepare(Source::WikidataPayments, &data.to_string(), "global").is_err());
    }
}
