use anyhow::{Context, Result, bail, ensure};
use calamine::{Reader, Xlsx};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    io::{Cursor, Read},
    time::Duration,
};
use ultrafinance_core::datasets::{Source, financial as fi};

pub async fn download(source: Source) -> Result<(&'static str, String, Vec<u8>)> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent(
            "Ultrafinance/0.1 (https://ultrafinance.app; financial institution source importer)",
        )
        .build()?;
    eprintln!("Downloading {} financial institutions", source.name());
    match source {
        Source::WikidataPayments => payment_providers(&client).await,
        Source::Fdic => {
            let mut all = vec![];
            let mut index = None;
            let mut expected = None;
            loop {
                let mut url = reqwest::Url::parse("https://api.fdic.gov/banks/institutions")?;
                url.query_pairs_mut().extend_pairs([
                    ("filters", "ACTIVE:1"),
                    ("fields", "CERT,NAME,ACTIVE,WEBADDR"),
                    ("format", "json"),
                    ("limit", "10000"),
                    ("sort_by", "CERT"),
                    ("sort_order", "ASC"),
                    ("offset", &all.len().to_string()),
                ]);
                let page: Value =
                    serde_json::from_slice(&super::sources::fetch(&client, url.as_str()).await?)?;
                let total = page["meta"]["total"]
                    .as_u64()
                    .context("FDIC total missing")?;
                let version = page["meta"]["index"]["name"]
                    .as_str()
                    .context("FDIC index missing")?
                    .to_owned();
                ensure!(total <= 100_000, "FDIC result exceeds supported size");
                if let Some(expected) = expected {
                    ensure!(
                        expected == total && index.as_ref() == Some(&version),
                        "FDIC changed during pagination; retry"
                    );
                }
                expected = Some(total);
                index = Some(version);
                let rows = page["data"].as_array().context("FDIC rows missing")?;
                ensure!(!rows.is_empty(), "FDIC returned an empty page");
                all.extend(rows.iter().cloned());
                if all.len() >= total as usize {
                    ensure!(all.len() == total as usize, "FDIC pagination overflow");
                    break;
                }
            }
            Ok((
                "input.json",
                "https://api.fdic.gov/banks/institutions?filters=ACTIVE%3A1".into(),
                serde_json::to_vec(
                    &json!({"meta":{"total":all.len(),"index":{"name":index}},"data":all}),
                )?,
            ))
        }
        Source::Ncua => {
            let html = super::sources::fetch(&client, fi::NCUA_URL).await?;
            let url = ncua_link(std::str::from_utf8(&html)?)?;
            let bytes = super::sources::fetch(&client, &url).await?;
            Ok(("input.csv", url, ncua_csv(&bytes)?))
        }
        Source::Wikidata => {
            let (url, bytes) = wikidata(&client).await?;
            Ok(("input.json", url, bytes))
        }
        Source::Gleif => {
            let (selection_url, bytes) = wikidata(&client).await?;
            let selection: Value = serde_json::from_slice(&bytes)?;
            let records = fi::wikidata(&selection)?;
            let mut ids = BTreeSet::new();
            for r in records {
                for lei in r.raw["leis"].as_array().unwrap() {
                    ids.insert(lei.as_str().unwrap().to_owned());
                }
            }
            ensure!(!ids.is_empty(), "Wikidata selection contains no LEIs");
            let ids: Vec<_> = ids.into_iter().collect();
            let mut all = vec![];
            for chunk in ids.chunks(100) {
                let mut url = reqwest::Url::parse("https://api.gleif.org/api/v1/lei-records")?;
                url.query_pairs_mut()
                    .append_pair("filter[lei]", &chunk.join(","))
                    .append_pair("page[size]", "100");
                let page: Value =
                    serde_json::from_slice(&super::sources::fetch(&client, url.as_str()).await?)?;
                let rows = page["data"].as_array().context("GLEIF rows missing")?;
                ensure!(
                    page["links"]["next"].is_null() && rows.len() == chunk.len(),
                    "GLEIF did not return every selected LEI; snapshot not published"
                );
                let returned: BTreeSet<_> = rows.iter().filter_map(|v| v["id"].as_str()).collect();
                ensure!(
                    returned == chunk.iter().map(String::as_str).collect(),
                    "GLEIF returned unexpected identities"
                );
                all.extend(rows.iter().cloned());
                eprintln!(
                    "GLEIF: fetched {} of {} selected LEIs",
                    all.len(),
                    ids.len()
                );
            }
            Ok((
                "input.json",
                fi::GLEIF_URL.into(),
                serde_json::to_vec(
                    &json!({"complete":true,"selection_url":selection_url,"selection":selection,"data":all}),
                )?,
            ))
        }
        _ => bail!("unsupported financial institution download"),
    }
}
async fn wikidata(client: &reqwest::Client) -> Result<(String, Vec<u8>)> {
    let mut url = reqwest::Url::parse("https://query.wikidata.org/sparql")?;
    url.query_pairs_mut()
        .append_pair("query", fi::WIKIDATA_QUERY)
        .append_pair("format", "json");
    let bytes = super::sources::fetch(client, url.as_str()).await?;
    fi::prepare(Source::Wikidata, std::str::from_utf8(&bytes)?, "global")?;
    Ok((url.into(), bytes))
}
fn ncua_link(html: &str) -> Result<String> {
    let document = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse("a[href]").unwrap();
    let links: BTreeSet<_> = document
        .select(&selector)
        .filter_map(|a| a.value().attr("href"))
        .filter_map(|href| reqwest::Url::parse(fi::NCUA_URL).ok()?.join(href).ok())
        .filter(|u| {
            u.scheme() == "https"
                && u.host_str() == Some("ncua.gov")
                && u.path().starts_with(
                    "/files/publications/analysis/federally-insured-credit-union-list-",
                )
                && u.path().ends_with(".zip")
        })
        .map(String::from)
        .collect();
    ensure!(
        links.len() == 1,
        "NCUA active-credit-union download link missing or ambiguous"
    );
    Ok(links.into_iter().next().unwrap())
}
fn ncua_csv(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    let names: Vec<_> = archive
        .file_names()
        .filter(|n| n.ends_with(".xlsx"))
        .map(str::to_owned)
        .collect();
    ensure!(
        names.len() == 1,
        "NCUA archive must contain exactly one workbook"
    );
    let mut workbook = Vec::new();
    let entry = archive.by_name(&names[0])?;
    ensure!(entry.size() <= 32 * 1024 * 1024, "NCUA workbook too large");
    entry
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut workbook)?;
    ensure!(
        workbook.len() <= 32 * 1024 * 1024,
        "NCUA workbook too large"
    );
    let inner = zip::ZipArchive::new(Cursor::new(&workbook))?;
    ensure!(
        inner
            .decompressed_size()
            .is_some_and(|n| n <= 128 * 1024 * 1024),
        "NCUA expanded workbook too large"
    );
    let mut xlsx: Xlsx<_> = Xlsx::new(Cursor::new(workbook))?;
    let sheet = xlsx
        .worksheet_range_at(0)
        .context("NCUA workbook has no worksheet")??;
    let mut csv = csv::Writer::from_writer(vec![]);
    for row in sheet.rows() {
        csv.write_record(row.iter().map(ToString::to_string))?;
    }
    let bytes = csv.into_inner()?;
    fi::prepare(Source::Ncua, std::str::from_utf8(&bytes)?, "global")?;
    Ok(bytes)
}

async fn payment_providers(client: &reqwest::Client) -> Result<(&'static str, String, Vec<u8>)> {
    let mut category_members = vec![];
    let mut continuation = None::<String>;
    let mut continuations = BTreeSet::new();
    loop {
        let mut url = reqwest::Url::parse("https://en.wikipedia.org/w/api.php")?;
        url.query_pairs_mut().extend_pairs([
            ("action", "query"),
            ("list", "categorymembers"),
            ("cmtitle", "Category:Payment service providers"),
            ("cmlimit", "500"),
            ("cmnamespace", "0"),
            ("format", "json"),
        ]);
        if let Some(value) = &continuation {
            url.query_pairs_mut().append_pair("cmcontinue", value);
        }
        let page: Value =
            serde_json::from_slice(&super::sources::fetch(client, url.as_str()).await?)?;
        category_members.extend(
            page["query"]["categorymembers"]
                .as_array()
                .context("Wikipedia category members missing")?
                .iter()
                .cloned(),
        );
        ensure!(
            category_members.len() <= 10_000,
            "payment-provider category exceeds supported size"
        );
        continuation = page["continue"]["cmcontinue"].as_str().map(str::to_owned);
        if let Some(value) = &continuation {
            ensure!(
                continuations.insert(value.clone()),
                "repeated category continuation"
            );
        } else {
            break;
        }
    }
    eprintln!(
        "Payment providers: selected {} category pages",
        category_members.len()
    );
    ensure!(
        !category_members.is_empty(),
        "empty payment-provider category"
    );
    let mut members = vec![];
    for chunk in category_members.chunks(50) {
        let ids = chunk
            .iter()
            .map(|r| {
                r["pageid"]
                    .as_u64()
                    .context("category page ID missing")
                    .map(|id| id.to_string())
            })
            .collect::<Result<Vec<_>>>()?
            .join("|");
        let mut url = reqwest::Url::parse("https://en.wikipedia.org/w/api.php")?;
        url.query_pairs_mut().extend_pairs([
            ("action", "query"),
            ("prop", "pageprops"),
            ("ppprop", "wikibase_item"),
            ("pageids", ids.as_str()),
            ("format", "json"),
        ]);
        let page: Value =
            serde_json::from_slice(&super::sources::fetch(client, url.as_str()).await?)?;
        ensure!(
            page["continue"].is_null(),
            "incomplete Wikipedia page mappings"
        );
        let pages = page["query"]["pages"]
            .as_object()
            .context("Wikipedia pages missing")?;
        ensure!(
            pages.len() == chunk.len(),
            "incomplete Wikipedia page mappings"
        );
        for page in pages.values() {
            ensure!(
                page["missing"].is_null(),
                "category changed during download; retry"
            );
            if let Some(qid) = page["pageprops"]["wikibase_item"].as_str() {
                ensure!(
                    qid.starts_with('Q')
                        && qid.len() > 1
                        && qid[1..].bytes().all(|b| b.is_ascii_digit()),
                    "invalid Wikidata page mapping"
                );
                members.push(json!({"title":page["title"],"qid":qid}));
            }
        }
    }
    let ids: BTreeSet<_> = members.iter().filter_map(|m| m["qid"].as_str()).collect();
    let ids: Vec<_> = ids.into_iter().collect();
    ensure!(!ids.is_empty(), "category has no Wikidata identities");
    let mut bindings = vec![];
    eprintln!(
        "Payment providers: resolving {} Wikidata identities",
        ids.len()
    );
    for chunk in ids.chunks(40) {
        let values = chunk
            .iter()
            .map(|id| format!("wd:{id}"))
            .collect::<Vec<_>>()
            .join(" ");
        let classes = fi::PAYMENT_PROVIDER_CLASSES
            .iter()
            .map(|id| format!("wd:{id}"))
            .collect::<Vec<_>>()
            .join(" ");
        let query = format!(
            r#"SELECT DISTINCT ?item ?name ?country ?website ?lei ?alias ?paymentProviderCategory ?providerKind ?instanceOf ?dissolved WHERE {{
          hint:Query hint:optimizer "None" .
          VALUES ?item {{ {values} }}
          BIND(<{}> AS ?paymentProviderCategory)
          OPTIONAL {{ ?item rdfs:label ?englishName FILTER(LANG(?englishName)="en") }}
          OPTIONAL {{ ?item rdfs:label ?defaultName FILTER(LANG(?defaultName)="mul") }}
          BIND(COALESCE(?englishName, ?defaultName) AS ?name)
          OPTIONAL {{ ?item wdt:P576 ?dissolved }}
          OPTIONAL {{ ?item wdt:P31 ?instanceOf }}
          OPTIONAL {{
            ?item wdt:P31 ?class .
            VALUES ?providerKind {{ {classes} }}
            ?class wdt:P279* ?providerKind . hint:Prior hint:gearing "forward" .
          }}
          OPTIONAL {{ ?item wdt:P17/wdt:P297 ?country }}
          OPTIONAL {{ ?item wdt:P856 ?website }} OPTIONAL {{ ?item wdt:P1278 ?lei }}
          OPTIONAL {{ ?item skos:altLabel ?alias FILTER(LANG(?alias)="en") }}
        }} ORDER BY ?item ?name ?country ?website ?lei ?alias ?providerKind ?instanceOf ?dissolved"#,
            fi::PAYMENTS_URL
        );
        let mut url = reqwest::Url::parse("https://query.wikidata.org/sparql")?;
        url.query_pairs_mut()
            .append_pair("query", &query)
            .append_pair("format", "json");
        let page: Value =
            serde_json::from_slice(&super::sources::fetch(client, url.as_str()).await?)?;
        bindings.extend(
            page["results"]["bindings"]
                .as_array()
                .context("payment-provider bindings missing")?
                .iter()
                .cloned(),
        );
    }
    let mut data = json!({"selection_version":2,"category":{"query":{"categorymembers":category_members}},"members":members,"results":{"bindings":bindings}});
    let (records, excluded) = fi::payment_provider_selection(&data)?;
    eprintln!(
        "Payment providers: imported {} identities; excluded {} category pages",
        records.len(),
        excluded.len()
    );
    for entry in &excluded {
        eprintln!(
            "Payment providers: excluded {} ({}): {}",
            entry["title"].as_str().unwrap_or("?"),
            entry["qid"].as_str().unwrap_or("no QID"),
            entry["reason"].as_str().unwrap_or("?")
        );
    }
    data["selection_report"] = json!({"imported":records.len(),"excluded":excluded});
    fi::prepare(Source::WikidataPayments, &data.to_string(), "global")?;
    Ok((
        "input.json",
        fi::PAYMENTS_URL.into(),
        serde_json::to_vec(&data)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ncua_discovery_rejects_missing_ambiguous_and_external_links() {
        let path = "/files/publications/analysis/federally-insured-credit-union-list-june-2026.zip";
        assert_eq!(
            ncua_link(&format!("<a href='{path}'>List</a>")).unwrap(),
            format!("https://ncua.gov{path}")
        );
        assert!(ncua_link(&format!("<a href='https://example.com{path}'>List</a>")).is_err());
        assert!(
            ncua_link(&format!(
                "<a href='{path}'>List</a><a href='{}'>Other</a>",
                path.replace("june", "march")
            ))
            .is_err()
        );
        assert!(ncua_csv(b"not a ZIP").is_err());
    }
}
