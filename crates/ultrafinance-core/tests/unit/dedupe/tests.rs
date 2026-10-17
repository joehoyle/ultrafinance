#[tokio::test]
async fn checkpoint_reuses_completed_requests_after_provider_payment_failure() -> Result<()> {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    let server = std::thread::spawn(move || {
        for status in [200, 402, 200] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut bytes = Vec::new();
            let request: Value = loop {
                let mut buffer = [0; 4096];
                let n = stream.read(&mut buffer).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if bytes.len() >= end + 4 + length {
                        break serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
                    }
                }
            };
            let answers: serde_json::Map<String, Value> = request["questions"].as_object().unwrap().keys().map(|name| (name.clone(), json!({"type":"choice","choice":"different","confidence":1.0,"probabilities":{"same":0.0,"different":1.0,"related":0.0,"insufficient":0.0}}))).collect();
            let response = json!({"answers":answers}).to_string();
            write!(stream,"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).unwrap();
        }
    });
    let path = std::env::temp_dir().join(format!(
        "ultra-provider-checkpoint-{}.jsonl",
        uuid::Uuid::new_v4()
    ));
    let mut cache = crate::dedupe_cache::Cache::open(&path)?;
    let client = reqwest::Client::new();
    let first = BTreeMap::from([("pair_0".into(), json!({"evidence":"first"}))]);
    let second = BTreeMap::from([("pair_1".into(), json!({"evidence":"second"}))]);
    let completed = request_identity_answers(
        &client,
        &endpoint,
        "test",
        "model",
        &first,
        Some(&mut cache),
    )
    .await?;
    assert!(
        request_identity_answers(
            &client,
            &endpoint,
            "test",
            "model",
            &second,
            Some(&mut cache)
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("402")
    );
    drop(cache);
    let mut cache = crate::dedupe_cache::Cache::open(&path)?;
    assert_eq!(
        completed,
        request_identity_answers(
            &client,
            &endpoint,
            "test",
            "model",
            &first,
            Some(&mut cache)
        )
        .await?
    );
    assert_eq!(
        request_identity_answers(
            &client,
            &endpoint,
            "test",
            "model",
            &second,
            Some(&mut cache)
        )
        .await?
        .len(),
        1
    );
    server.join().unwrap();
    drop(cache);
    std::fs::remove_file(path)?;
    Ok(())
}
use super::*;
fn merchant(id: &str, name: &str, website: &str) -> Merchant {
    serde_json::from_value(json!({"id":id,"name":name,"website":website})).unwrap()
}
fn record(id: &str, name: &str, website: &str) -> SourceRecord {
    SourceRecord {
        source: "test".into(),
        external_id: id.into(),
        merchant: merchant(id, name, website),
        attribution: "test".into(),
        license: "test".into(),
        url: "https://example.org".into(),
        version: None,
        raw: json!({}),
    }
}
#[test]
fn import_plan_replays_ordered_merges_and_rejects_stale_or_invalid_plans() -> Result<()> {
    let db = MerchantStore::temporary()?;
    db.import(&[
        record("a", "Brand", "https://example.com"),
        record("b", "Brand", "https://example.com"),
        record("c", "Brand", "https://example.com"),
    ])?;
    let a = db.resolve_source("test", "a")?.unwrap();
    let b = db.resolve_source("test", "b")?.unwrap();
    let c = db.resolve_source("test", "c")?.unwrap();
    let revision = db.dedupe_snapshot()?.revision.unwrap();
    let plan = |groups: Value| json!({"reconciliation":{"dedupe":{"dry_run":true,"errors":0,"catalog_revision":revision,"groups":groups}}});
    // A failure in a later event must roll back the first merge as well.
    assert!(apply_import_plan(&db, &plan(json!([[a, b], [c, "missing"]]))).is_err());
    assert_eq!(db.stats()?.total, 3);
    assert_eq!(db.resolve_source("test", "b")?, Some(b.clone()));
    assert_eq!(db.dedupe_snapshot()?.revision, Some(revision));
    assert!(apply_import_plan(&db, &plan(json!([[a, a]]))).is_err());
    apply_import_plan(&db, &plan(json!([[a, b], [c, a]])))?;
    assert_eq!(db.stats()?.total, 1);
    for id in [&a, &b] {
        assert_eq!(db.resolve_merchant_id(id)?, c);
    }
    for id in ["a", "b", "c"] {
        assert_eq!(db.resolve_source("test", id)?, Some(c.clone()));
    }
    assert!(
        apply_import_plan(&db, &plan(json!([[c, a]])))
            .unwrap_err()
            .to_string()
            .contains("catalog changed")
    );
    Ok(())
}
#[test]
fn import_plan_preserves_conflicting_manual_identities() -> Result<()> {
    let db = MerchantStore::temporary()?;
    db.put(&merchant("a", "Brand", "https://example.com"))?;
    db.put(&merchant("b", "Brand", "https://example.com"))?;
    let revision = db.dedupe_snapshot()?.revision;
    let plan = json!({"reconciliation":{"dedupe":{"dry_run":true,"errors":0,"catalog_revision":revision,"groups":[["a","b"]]}}});
    assert!(
        apply_import_plan(&db, &plan)
            .unwrap_err()
            .to_string()
            .contains("conflicting manual")
    );
    assert_eq!(db.stats()?.total, 2);
    Ok(())
}
#[tokio::test]
#[ignore = "fresh-write performance benchmark; uses an isolated PostgreSQL database"]
async fn benchmark_fresh_merchant_import() -> Result<()> {
    let count: usize = std::env::var("ULTRAFINANCE_IMPORT_BENCH_RECORDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10000);
    let db = MerchantStore::temporary()?;
    let records: Vec<SourceRecord> = if let Ok(path) =
        std::env::var("ULTRAFINANCE_IMPORT_BENCH_INPUT")
    {
        crate::datasets::read_selected_records(std::path::Path::new(&path), Some(count as u32))?.0
    } else {
        (0..count).map(|i| {
        let mut r = record(&format!("place-{i:08}"), &format!("Merchant {i}"), "https://benchmark.example");
        r.merchant.markets = vec!["CA".into()];
        r.merchant.aliases = vec![format!("BANK MERCHANT {i}")];
        r.raw = json!({"places":[{"country":"CA","address":format!("{i} Main Street"),"locality":"Montreal"}]});
        r
    }).collect()
    };
    let count = records.len();
    let sample = records[0].clone();
    let start = std::time::Instant::now();
    let result = import(db.clone(), records, ImportOptions::default()).await?;
    let elapsed = start.elapsed();
    assert_eq!(result.delta.added, count);
    assert_eq!(db.stats()?.total, count - result.dedupe.candidates);
    let sample_id = db
        .resolve_source(&sample.source, &sample.external_id)?
        .unwrap();
    let m = db.get(&sample_id)?.unwrap();
    for alias in sample.merchant.aliases {
        assert!(m.aliases.contains(&alias));
    }
    for market in sample.merchant.markets {
        assert!(m.markets.contains(&market));
    }
    eprintln!(
        "FRESH_IMPORT_BENCH records={count} elapsed_seconds={:.3} records_per_second={:.1}",
        elapsed.as_secs_f64(),
        count as f64 / elapsed.as_secs_f64()
    );
    Ok(())
}
#[tokio::test]
#[ignore = "comparative throughput benchmark; isolated PostgreSQL databases"]
async fn benchmark_streaming_against_snapshot_import() -> Result<()> {
    let count = std::env::var("ULTRAFINANCE_IMPORT_BENCH_RECORDS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(10000);
    let repeats = std::env::var("ULTRAFINANCE_IMPORT_BENCH_REPEATS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(3);
    let root = std::env::temp_dir().join(format!("ultra-throughput-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root)?;
    let mut cases: Vec<(&str, Vec<SourceRecord>)> = vec![
        (
            "unique",
            (0..count)
                .map(|i| {
                    record(
                        &i.to_string(),
                        &format!("Merchant {i}"),
                        "https://benchmark.example",
                    )
                })
                .collect(),
        ),
        (
            "chains",
            (0..count)
                .map(|i| {
                    let mut r = record(
                        &i.to_string(),
                        &format!("Chain {}", i % 200),
                        "https://benchmark.example",
                    );
                    r.merchant.website = None;
                    r
                })
                .collect(),
        ),
    ];
    if let Ok(path) = std::env::var("ULTRAFINANCE_IMPORT_BENCH_INPUT") {
        let path = std::path::Path::new(&path);
        let mut records = if path.extension().is_some_and(|s| s == "csv") {
            crate::datasets::prepare(
                crate::datasets::Source::Foursquare,
                &std::fs::read_to_string(path)?,
                None,
                "ca",
            )?
            .records
        } else {
            crate::datasets::read_selected_records(path, Some(count as u32))?.0
        };
        records.truncate(count);
        cases.push(("foursquare", records));
    }
    if let Ok(case) = std::env::var("ULTRAFINANCE_IMPORT_BENCH_CASE") {
        cases.retain(|(label, _)| *label == case);
    }
    for (label, records) in cases {
        let path = root.join(format!("{label}.json"));
        std::fs::write(&path, serde_json::to_vec(&records)?)?;
        let count = records.len();
        let mut expected = std::collections::HashSet::new();
        for (i, r) in records.iter().enumerate() {
            expected.insert(
                deterministic_key(&r.merchant)
                    .map(|k| format!("{k:?}"))
                    .unwrap_or_else(|| format!("unmatched-{i}")),
            );
        }
        for repeat in 0..repeats {
            // Alternate order so caches do not consistently favour one strategy.
            for streaming in if repeat % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let db = MerchantStore::temporary()?;
                for phase in ["fresh", "refresh"] {
                    let start = std::time::Instant::now();
                    let delta = if streaming {
                        import_file(
                            db.clone(),
                            path.clone(),
                            None,
                            ImportOptions {
                                chunk_size: std::env::var("ULTRAFINANCE_IMPORT_BENCH_CHUNK_SIZE")
                                    .ok()
                                    .and_then(|v| v.parse().ok())
                                    .unwrap_or(DEFAULT_IMPORT_CHUNK_SIZE),
                                ..Default::default()
                            },
                        )
                        .await?
                        .0
                        .delta
                    } else {
                        let records = crate::datasets::read_selected_records(&path, None)?.0;
                        let snapshot = db.dedupe_snapshot()?;
                        let (staged, ids, touched) = stage(&snapshot, &records)?;
                        let report = deterministic_plan(&snapshot, &staged, &touched, false);
                        let groups = report.groups.clone();
                        db.apply_reconciled_import(&snapshot, &staged, &records, &ids, &groups)?
                            .0
                    };
                    let elapsed = start.elapsed().as_secs_f64();
                    if phase == "fresh" {
                        assert_eq!(delta.added, count);
                    } else {
                        assert_eq!(delta.unchanged, count);
                    }
                    assert_eq!(db.stats()?.total, expected.len());
                    eprintln!(
                        "THROUGHPUT_BENCH case={label} phase={phase} strategy={} repeat={repeat} records={count} seconds={elapsed:.6} records_per_second={:.1}",
                        if streaming { "streaming" } else { "snapshot" },
                        count as f64 / elapsed
                    );
                }
            }
        }
    }
    std::fs::remove_dir_all(root)?;
    Ok(())
}
fn answer(choice: &str, probability: f64, confidence: f64) -> Value {
    json!({"type":"choice","choice":choice,"confidence":confidence,"probabilities":{"same":probability,"related":1.-probability,"different":0.,"insufficient":0.}})
}
#[tokio::test]
async fn identity_requests_stream_all_pairs_within_actual_byte_limits() -> Result<()> {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    let mut merchants: Vec<_> = (0..7)
        .map(|i| {
            merchant(
                &i.to_string(),
                &format!("Brand {i}"),
                &format!("https://brand{i}.test"),
            )
        })
        .collect();
    for m in &mut merchants {
        m.aliases.push("é".repeat(4000));
    }
    let snapshot = Snapshot {
        merchants,
        manual: vec![],
        sources: vec![],
        locations: vec![],
        redirects: vec![],
        location_redirects: vec![],
        revision: None,
    };
    let pairs: Vec<_> = (0..7)
        .flat_map(|a| (a + 1..7).map(move |b| (a, b)))
        .collect();
    let total = pairs.len();
    let server = std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let mut seen = BTreeSet::new();
        let mut requests = 0;
        while seen.len() < total {
            let mut stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(started.elapsed() < std::time::Duration::from_secs(30));
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("{error}"),
            };
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            let body: Value = loop {
                let n = stream.read(&mut buffer).unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buffer[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    assert!(length <= 48 * 1024);
                    if request.len() >= end + 4 + length {
                        break serde_json::from_slice(&request[end + 4..end + 4 + length]).unwrap();
                    }
                }
            };
            let questions = body["questions"].as_object().unwrap();
            assert!(questions.len() <= 32);
            let mut answers = serde_json::Map::new();
            for (key, q) in questions {
                assert!(seen.insert(key.clone()));
                assert!(serde_json::to_vec(q).unwrap().len() <= 24 * 1024);
                answers.insert(key.clone(),json!({"type":"choice","choice":"different","confidence":1.0,"probabilities":{"same":0.0,"related":0.0,"different":1.0,"insufficient":0.0}}));
            }
            let response = json!({"answers":answers}).to_string();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response).unwrap();
            requests += 1;
        }
        assert!(requests > 1);
    });
    let report = evaluate(
        &snapshot,
        pairs,
        Some("test".into()),
        "téšt-💡".into(),
        0.98,
        true,
        total,
        &endpoint,
        None,
    )
    .await?;
    server.join().unwrap();
    assert_eq!(report.decisions.len(), total);
    assert_eq!(report.errors, 0);
    assert!(report.groups.is_empty());
    Ok(())
}
#[test]
fn source_heavy_questions_preserve_identity_without_raw_or_media_payloads() {
    let mut a = merchant("a", "Network", "https://network.example");
    a.aliases = vec!["BANK NETWORK".into()];
    a.logo_url = Some(format!("https://media.example/{}", "x".repeat(30000)));
    let b = merchant("b", "Network Rewards", "https://network.example");
    let mut sources = Vec::new();
    for index in 0..22 {
        let mut row = record(&index.to_string(), "Network", "https://network.example");
        row.merchant.aliases = vec![format!("NETWORK SOURCE {index}")];
        row.merchant.markets = vec!["CA".into()];
        row.raw = json!({"irrelevant_payload":"x".repeat(30000)});
        sources.push(("a".into(), row));
    }
    // Repeated places must not repeat identical brand facts or publisher
    // schema URLs in a provider question. The audit retains every record.
    for index in 0..1000 {
        let mut duplicate = sources[0].1.clone();
        duplicate.external_id = format!("duplicate-{index}");
        duplicate.merchant.website = Some(format!("https://network.example/locations/{index}"));
        sources.push(("a".into(), duplicate));
    }
    let snapshot = Snapshot {
        merchants: vec![a, b],
        manual: vec![],
        sources,
        locations: vec![],
        redirects: vec![],
        location_redirects: vec![],
        revision: None,
    };
    let q = question(&snapshot, 0, 1);
    let left = &q["instructions"]["left"];
    assert_eq!(left["sources"].as_array().unwrap().len(), 22);
    assert_eq!(
        left["sources"][21]["merchant"]["aliases"][0],
        "NETWORK SOURCE 21"
    );
    assert_eq!(left["sources"][0]["merchant"]["markets"][0], "CA");
    assert!(left["sources"][0].get("raw").is_none());
    assert!(left["sources"][0].get("external_id").is_none());
    assert!(left["sources"][0].get("url").is_none());
    assert!(left.get("id").is_none());
    assert_eq!(snapshot.sources.len(), 1022);
    assert!(left["merchant"].get("logo_url").is_none());
    assert!(serde_json::to_vec(&q).unwrap().len() < 24 * 1024);
    assert_eq!(
        snapshot.sources[0].1.raw["irrelevant_payload"]
            .as_str()
            .unwrap()
            .len(),
        30000
    );
}

#[test]
fn identity_compacts_business_urls_but_preserves_shared_platform_paths() {
    let owned = identity(&merchant(
        "a",
        "Northstar",
        "https://locations.northstar.test/stores/02232",
    ));
    assert_eq!(owned["website_host"], "locations.northstar.test");
    assert!(owned.get("website").is_none());
    let tenant = identity(&merchant(
        "b",
        "Northstar",
        "https://facebook.com/northstar-granby",
    ));
    assert_eq!(tenant["website"], "https://facebook.com/northstar-granby");
    assert!(tenant.get("website_host").is_none());
    let unrelated = identity(&merchant("c", "Other", "https://northstar.test/other"));
    assert_eq!(unrelated["website_host"], "northstar.test");
    assert_eq!(unrelated["name"], "Other");
}

#[test]
fn shared_platform_can_identify_its_owner_but_not_its_tenants() {
    let owner = merchant("a", "Facebook", "https://facebook.com");
    assert!(same_identity_by_rule(
        &owner,
        &merchant("b", "FACEBOOK Inc", "https://www.facebook.com/about")
    ));
    for name in ["Cafe", "Facebook Cafe", "Book", "Face"] {
        assert!(
            deterministic_key(&merchant("tenant", name, "https://facebook.com/cafe")).is_none()
        );
    }
    for domain in ["yp.ca", "pj.ca", "yellowpages.ca", "pagesjaunes.ca"] {
        assert!(
            deterministic_key(&merchant(
                "tenant",
                "Northstar Books",
                &format!("https://{domain}/business/northstar")
            ))
            .is_none()
        );
    }
    assert!(
        deterministic_key(&merchant(
            "tenant",
            "Facebook",
            "https://tenant.facebook.com"
        ))
        .is_none()
    );
}

#[test]
fn website_rule_requires_same_brand_and_a_real_website_when_present() {
    let a = merchant("a", "Uber", "https://uber.com");
    assert!(same_identity_by_rule(
        &a,
        &merchant("b", "UBER", "http://www.uber.com/ca/")
    ));
    assert!(!same_identity_by_rule(
        &a,
        &merchant("b", "Uber Eats", "https://uber.com")
    ));
    assert!(!same_identity_by_rule(
        &a,
        &merchant("b", "Uber", "https://other.com")
    ));
    let mut missing = a.clone();
    missing.website = None;
    assert!(same_identity_by_rule(&missing, &missing));
    assert!(!same_identity_by_rule(&a, &missing));
}

#[test]
fn blank_websites_merge_only_complete_normalized_names() {
    let blank = |name: &str, website: Option<&str>| {
        let mut m = merchant("test", name, "https://unused.test");
        m.website = website.map(String::from);
        m
    };
    let a = blank("Café Starbucks", None);
    let b = blank(" CAFE  STARBUCKS ", Some("  "));
    assert!(same_identity_by_rule(&a, &b));
    for other in [
        blank("Cafe Starbucks LLC", None),
        blank("Cafe Starbucks Toronto", None),
        blank("Cafe Starbucks", Some("https://starbucks.com")),
        blank("Cafe Starbucks", Some("https://facebook.com/starbucks")),
        blank("Cafe Starbucks", Some("invalid")),
    ] {
        assert!(!same_identity_by_rule(&a, &other));
    }
    assert!(!same_identity_by_rule(
        &blank("!!!", None),
        &blank("", None)
    ));
    assert_eq!(
        rule_answer(&a)["rule"],
        "same_normalized_name_and_both_websites_blank"
    );
}

#[tokio::test]
async fn blank_website_import_merges_existing_places_and_survives_refresh() -> Result<()> {
    let db = MerchantStore::temporary()?;
    let records: Vec<_> = (0..200)
        .map(|i| {
            let mut r = record(&format!("place:{i}"), "Starbucks", "https://unused.test");
            r.source = "foursquare".into();
            r.merchant.website = None;
            r.merchant.markets = vec![if i % 2 == 0 { "CA" } else { "US" }.into()];
            r.raw =
                json!({"places":[{"fsq_place_id":i.to_string(),"address":format!("{i} Main St")}]});
            r
        })
        .collect();
    // All import paths now use provider IDs and the current merchant state.
    db.import(&records[..2])?;
    let survivor = db
        .resolve_source("foursquare", &records[0].external_id)?
        .unwrap();
    let preview = import(
        db.clone(),
        records.clone(),
        ImportOptions {
            dry_run: true,
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(preview.dedupe.candidates, 198);
    assert_eq!(db.source_records("foursquare", None, 1000, 0)?.len(), 2);
    import(db.clone(), records.clone(), ImportOptions::default()).await?;
    assert_eq!(db.stats()?.total, 1);
    for r in &records {
        assert_eq!(
            db.resolve_source("foursquare", &r.external_id)?.as_ref(),
            Some(&survivor)
        );
    }
    let refresh = import(db.clone(), records, ImportOptions::default()).await?;
    assert_eq!(refresh.delta.unchanged, 200);
    assert!(refresh.dedupe.groups.is_empty());
    assert!(db.dedupe_snapshot()?.sources.is_empty());
    Ok(())
}

#[tokio::test]
async fn blank_website_dedupe_uses_rules_and_preserves_manual_conflicts() -> Result<()> {
    let db = MerchantStore::temporary()?;
    let mut a = record("a", "Starbucks", "https://unused.test");
    a.merchant.website = None;
    let mut b = a.clone();
    b.external_id = "b".into();
    db.import(&[a.clone(), b])?;
    let report = run_at(db.clone(), None, "test".into(), 0.98, false, 10, "unused").await?;
    assert_eq!(report.groups.len(), 1);
    assert!(report.decisions.iter().all(|d| d.method == "rule"));
    assert_eq!(db.stats()?.total, 1);
    for id in ["manual-a", "manual-b"] {
        let mut m = a.merchant.clone();
        m.id = id.into();
        db.put(&m)?;
    }
    a.external_id = "c".into();
    let report = import(db.clone(), vec![a], ImportOptions::default()).await?;
    assert!(report.dedupe.groups.is_empty());
    assert_eq!(db.stats()?.total, 4);
    Ok(())
}

#[tokio::test]
async fn rules_merge_missing_and_different_markets_without_jev_and_survive_refresh() -> Result<()> {
    let db = MerchantStore::temporary()?;
    let a = record("a", "Uber", "https://uber.com");
    let mut b = record("b", "Uber", "https://www.uber.com/us/");
    b.merchant.markets = vec!["US".into()];
    let mut c = record("c", "UBER", "http://uber.com/ca/");
    c.merchant.markets = vec!["CA".into()];
    let records = vec![a, b, c];
    db.import(&records)?;
    let preview = run_at(db.clone(), None, "test".into(), 0.98, true, 10, "unused").await?;
    assert_eq!(preview.groups.len(), 1);
    assert_eq!(preview.decisions.len(), 3);
    assert!(
        preview
            .decisions
            .iter()
            .all(|d| d.accepted && d.method == "rule")
    );
    assert_eq!(db.stats()?.total, 3);
    let report = run_at(db.clone(), None, "test".into(), 0.98, false, 10, "unused").await?;
    assert!(report.run_id.is_some());
    assert_eq!(db.stats()?.total, 1);
    let id = &report.groups[0][0];
    for external in ["a", "b", "c"] {
        assert_eq!(db.resolve_source("test", external)?.as_ref(), Some(id));
    }
    let merged = db.get(id)?.unwrap();
    assert_eq!(merged.markets, vec!["CA", "US"]);
    db.import(&records)?;
    assert_eq!(db.stats()?.total, 1);
    assert_eq!(db.get(id)?.unwrap().markets, vec!["CA", "US"]);
    Ok(())
}

#[tokio::test]
async fn import_reuses_established_ids_merges_batch_evidence_and_refreshes() -> Result<()> {
    let db = MerchantStore::temporary()?;
    let base = record("existing", "Uber", "https://uber.com");
    db.import(&[base])?;
    let existing = db.resolve_source("test", "existing")?.unwrap();
    let mut a = record("a", "UBER", "https://www.uber.com/ca");
    a.merchant.markets = vec!["CA".into()];
    a.merchant.aliases = vec!["UBER BILL".into()];
    a.raw = json!({"outlet":"Bromont"});
    let mut b = record("b", "Uber", "https://uber.com/us");
    b.merchant.markets = vec!["US".into()];
    b.merchant.logo_url = Some("https://uber.com/logo.png".into());
    b.merchant.logo_source = Some("test".into());
    let input = vec![a.clone(), b.clone()];
    let preview = import_at(
        db.clone(),
        input.clone(),
        ImportOptions {
            dry_run: true,
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(preview.dedupe.groups.len(), 1);
    assert_eq!(db.stats()?.total, 1);
    assert!(db.resolve_source("test", "a")?.is_none());
    let report = import_at(db.clone(), input.clone(), ImportOptions::default()).await?;
    assert_eq!(report.delta.added, 2);
    assert_eq!(db.stats()?.total, 1);
    assert!(
        db.dedupe_snapshot()?.redirects.is_empty(),
        "new source identities should link directly without transient merchant rows"
    );
    assert_eq!(report.dedupe.groups[0][0], existing);
    for key in ["a", "b"] {
        assert_eq!(db.resolve_source("test", key)?, Some(existing.clone()));
    }
    let merged = db.get(&existing)?.unwrap();
    assert_eq!(merged.markets, ["CA", "US"]);
    assert_eq!(merged.logo_url, b.merchant.logo_url);
    assert!(merged.aliases.contains(&"UBER BILL".into()));
    assert!(!db.search("UBER BILL", None, 10)?[0].trusted);
    let refresh = import_at(db.clone(), input, ImportOptions::default()).await?;
    assert_eq!(refresh.delta.unchanged, 2);
    assert!(refresh.dedupe.groups.is_empty());
    a.raw = json!({"outlet":"Toronto", "refreshed":true});
    let refresh = import_at(db.clone(), vec![a], ImportOptions::default()).await?;
    assert_eq!(refresh.delta.unchanged, 1);
    assert_eq!(refresh.delta.updated, 0);
    assert_eq!(db.resolve_source("test", "a")?, Some(existing));
    assert_eq!(db.stats()?.total, 1);
    Ok(())
}

#[test]
fn deterministic_identity_handles_legal_suffixes_but_preserves_distinct_brands() {
    let brand = merchant("a", "Café Brand, Inc.", "https://www.brand.test/ca/");
    assert_eq!(canonical_name("The Limited"), "the limited");
    assert_eq!(canonical_name("7-Eleven LLC"), "7 eleven");
    assert!(same_identity_by_rule(
        &brand,
        &merchant("b", "CAFE BRAND LLC", "http://brand.test/us")
    ));
    assert!(!same_identity_by_rule(
        &brand,
        &merchant("b", "Cafe Brand Plus", "https://brand.test")
    ));
    assert!(!same_identity_by_rule(
        &brand,
        &merchant("b", "Cafe Brand", "https://brand-other.test")
    ));
    assert!(!same_identity_by_rule(
        &merchant("a", "Cafe", "https://facebook.com/cafe"),
        &merchant("b", "Cafe", "https://facebook.com/other-cafe")
    ));
    assert!(!same_identity_by_rule(
        &merchant("a", "Cafe", "https://shop.wordpress.com"),
        &merchant("b", "Cafe", "https://shop.wordpress.com")
    ));
    assert!(!same_identity_by_rule(
        &merchant("a", "Cafe", "https://user:pass@brand.test"),
        &merchant("b", "Cafe", "https://brand.test")
    ));
}
#[test]
fn deterministic_large_chain_produces_linear_merge_audit_without_pair_budget() {
    let expected = Snapshot {
        merchants: vec![],
        manual: vec![],
        sources: vec![],
        locations: vec![],
        redirects: vec![],
        location_redirects: vec![],
        revision: None,
    };
    let mut staged = expected.clone();
    for i in 0..20000 {
        staged.merchants.push(merchant(
            &format!("mer_{i:05}"),
            "Brand Ltd",
            "https://brand.test",
        ));
    }
    let touched = staged.merchants.iter().map(|m| m.id.clone()).collect();
    let report = deterministic_plan(&expected, &staged, &touched, true);
    assert_eq!(report.groups.len(), 1);
    assert_eq!(report.groups[0].len(), 20000);
    assert_eq!(report.decisions.len(), 19999);
    assert_eq!(report.model, "deterministic");
    assert!(report.decisions.iter().all(|d| d.method == "rule"));
    for m in &mut staged.merchants {
        m.website = None;
    }
    let report = deterministic_plan(&expected, &staged, &touched, true);
    assert_eq!(report.groups[0].len(), 20000);
    assert_eq!(report.decisions.len(), 19999);
}

#[tokio::test]
async fn deterministic_import_keeps_ambiguous_names_platforms_and_manual_conflicts_separate()
-> Result<()> {
    let db = MerchantStore::temporary()?;
    let incoming = vec![
        record("a", "Cafe", "https://facebook.com/cafe-a"),
        record("b", "Cafe", "https://facebook.com/cafe-b"),
        record("c", "Brand", "https://brand.test"),
        record("d", "Brand Eats", "https://brand.test"),
        record("e", "Brand", "https://other.test"),
    ];
    let result = import(db.clone(), incoming, ImportOptions::default()).await?;
    assert!(result.dedupe.groups.is_empty());
    assert_eq!(db.stats()?.total, 5);
    db.put(&merchant("reviewed-a", "Brand", "https://brand.test"))?;
    db.put(&merchant("reviewed-b", "Brand Inc", "https://brand.test"))?;
    let result = import(
        db.clone(),
        vec![record("f", "Brand LLC", "https://brand.test")],
        ImportOptions::default(),
    )
    .await?;
    assert!(result.dedupe.groups.is_empty());
    assert_eq!(db.stats()?.total, 8);
    let before = db.dedupe_snapshot()?;
    let incoming = vec![record("g", "Brand Ltd", "https://brand.test")];
    let (staged, ids, touched) = stage(&before, &incoming)?;
    let report = deterministic_plan(&before, &staged, &touched, false);
    db.put(&merchant(
        "new-manual",
        "Unrelated",
        "https://unrelated.test",
    ))?;
    assert!(
        db.apply_reconciled_import(&before, &staged, &incoming, &ids, &report.groups,)
            .is_err()
    );
    assert!(db.resolve_source("test", "g")?.is_none());
    Ok(())
}

#[tokio::test]
async fn import_preserves_manual_corrections_and_locations_when_merging() -> Result<()> {
    let db = MerchantStore::temporary()?;
    let mut manual = merchant("manual", "Brand", "https://example.com");
    manual.logo_url = Some("https://example.com/verified.png".into());
    manual.logo_source = Some("manual".into());
    db.put(&manual)?;
    db.import(&[record("old", "Brand", "https://example.com")])?;
    let old = db.resolve_source("test", "old")?.unwrap();
    let location: LocationRecord = serde_json::from_value(
        json!({"source":"test","external_id":"outlet","aliases":[],"merchant":{"merchant_id":old},"location":{"id":null,"precision":"outlet","address":"1 Main St","city":"Toronto","region":"ON","postal_code":null,"country":"CA","store_number":null},"attribution":"test","license":"test","url":"https://example.com"}),
    )?;
    db.import_locations(&[location])?;
    let result = import_at(
        db.clone(),
        vec![record("new", "Brand", "https://example.com")],
        ImportOptions::default(),
    )
    .await?;
    // A focused import compares incoming identities to both existing records;
    // their existing pair is needed to consolidate the whole brand.
    assert!(!result.dedupe.groups.is_empty());
    assert_eq!(db.resolve_source("test", "new")?, Some("manual".into()));
    assert_eq!(db.get("manual")?.unwrap().logo_url, manual.logo_url);
    assert_eq!(db.stats()?.total, 1);
    assert_eq!(db.resolve_merchant_id(&old)?, "manual");
    assert_eq!(db.locations("manual")?.len(), 1);
    Ok(())
}

#[test]
fn answers_fail_closed_and_require_both_thresholds() {
    assert!(accepted(&answer("same", 0.99, 0.99), 0.98).unwrap());
    assert!(!accepted(&answer("related", 0.99, 0.99), 0.98).unwrap());
    assert!(!accepted(&answer("same", 0.97, 0.99), 0.98).unwrap());
    assert!(!accepted(&answer("same", 0.99, 0.97), 0.98).unwrap());
    assert!(accepted(&json!({"choice":"same"}), 0.98).is_err());
    assert!(accepted(&answer("same", 1.2, 0.99), 0.98).is_err());
}
#[test]
fn distribution_rounding_does_not_change_merge_thresholds() {
    let rounded = json!({"type":"choice", "choice":"same", "confidence":0.99,
        "probabilities":{"same":0.99,"related":0.02,"different":0.0,"insufficient":0.0}});
    assert!(accepted(&rounded, 0.98).unwrap());
    let low = json!({"type":"choice", "choice":"same", "confidence":0.99,
        "probabilities":{"same":0.97,"related":0.04,"different":0.0,"insufficient":0.0}});
    assert!(!accepted(&low, 0.98).unwrap());
    let invalid = json!({"type":"choice", "choice":"same", "confidence":0.99,
        "probabilities":{"same":0.99,"related":0.2,"different":0.0,"insufficient":0.0}});
    assert!(
        accepted(&invalid, 0.98)
            .unwrap_err()
            .to_string()
            .contains("sum to 1.190000")
    );
}

#[test]
fn groups_require_every_pair_and_keep_manual_conflicts_separate() -> Result<()> {
    let db = MerchantStore::temporary()?;
    for id in ["a", "b", "c"] {
        db.put(&merchant(id, "Brand", "https://example.com"))?;
    }
    let mut snapshot = db.dedupe_snapshot()?;
    snapshot.manual.clear();
    let decisions: Vec<_> = [("a", "b"), ("b", "c")]
        .into_iter()
        .map(|(a, b)| Decision {
            left: a.into(),
            right: b.into(),
            answer: Value::Null,
            accepted: true,
            method: "test".into(),
            error: None,
        })
        .collect();
    assert_eq!(groups(&snapshot, &decisions), vec![vec!["a", "b"]]);
    snapshot.manual = snapshot.merchants.clone();
    assert!(groups(&snapshot, &decisions).is_empty());
    Ok(())
}
#[test]
fn merge_preserves_sources_outlets_metadata_redirects_and_refresh() -> Result<()> {
    exercise_merge(MerchantStore::temporary()?)
}
#[test]
fn candidates_include_typos_but_do_not_treat_shared_domains_as_identity() -> Result<()> {
    let snapshot = Snapshot {
        merchants: vec![
            merchant("a", "Starbucks", "https://starbucks.com"),
            merchant("b", "Starbuks", "https://other.example.com"),
            merchant("c", "Coffee Rewards", "https://www.starbucks.com"),
        ],
        manual: vec![],
        sources: vec![],
        locations: vec![],
        redirects: vec![],
        location_redirects: vec![],
        revision: None,
    };
    let candidates = pairs(&snapshot);
    assert!(candidates.contains(&(0, 1)));
    assert!(candidates.contains(&(0, 2)));
    assert!(!accepted(&answer("related", 0.01, 0.99), 0.98)?);
    Ok(())
}
fn exercise_merge(db: MerchantStore) -> Result<()> {
    let mut a = record("a", "Brand", "https://example.com");
    a.merchant.markets = vec!["CA".into()];
    let mut b = record("b", "Brand Inc", "https://www.example.com");
    b.merchant.logo_url = Some("https://example.com/logo.png".into());
    b.merchant.logo_source = Some("test".into());
    b.merchant.aliases = vec!["BANK BRAND".into()];
    db.import(&[a.clone(), b.clone()])?;
    let target = db.resolve_source("test", "a")?.unwrap();
    let retired = db.resolve_source("test", "b")?.unwrap();
    let outlet: LocationRecord = serde_json::from_value(
        json!({"source":"test","external_id":"outlet","aliases":["BRAND TORONTO"],"merchant":{"merchant_id":retired},"location":{"id":null,"precision":"outlet","address":"1 Main St","city":"Toronto","region":"ON","postal_code":null,"country":"CA","store_number":null},"attribution":"test","license":"test","url":"https://example.com"}),
    )?;
    db.import_locations(std::slice::from_ref(&outlet))?;
    let before = db.dedupe_snapshot()?;
    db.apply_dedupe(&before, &[vec![target.clone(), retired.clone()]])?;
    assert_eq!(db.stats()?.total, 1);
    assert_eq!(db.resolve_source("test", "b")?, Some(target.clone()));
    assert_eq!(db.resolve_merchant_id(&retired)?, target);
    assert_eq!(db.locations(&target)?.len(), 1);
    assert!(
        matches!(&db.locations(&target)?[0].merchant,crate::location::MerchantReference::Local{merchant_id} if merchant_id==&target)
    );
    assert_eq!(db.locations(&retired)?.len(), 1);
    db.import_locations(&[outlet])?;
    assert_eq!(db.locations(&target)?.len(), 1);
    assert!(
        db.put(&merchant(&retired, "Brand", "https://example.com"))
            .is_err()
    );
    db.import(&[a, b])?;
    let merged = &db.list(None, 10, 0)?.merchants[0];
    assert!(merged.aliases.contains(&"BANK BRAND".into()));
    assert_eq!(
        merged.logo_url.as_deref(),
        Some("https://example.com/logo.png")
    );
    assert!(merged.markets.contains(&"CA".into()));
    assert!(!db.search("BANK BRAND", None, 10)?[0].trusted);
    assert!(db.apply_dedupe(&before, &[vec![target, retired]]).is_err());
    Ok(())
}
#[test]
fn redirects_follow_later_merges_and_last_source_cannot_be_unlinked() -> Result<()> {
    let db = MerchantStore::temporary()?;
    db.import(&[
        record("a", "Brand", "https://example.com"),
        record("b", "Brand", "https://example.com"),
        record("c", "Brand", "https://example.com"),
    ])?;
    let a = db.resolve_source("test", "a")?.unwrap();
    let b = db.resolve_source("test", "b")?.unwrap();
    let c = db.resolve_source("test", "c")?.unwrap();
    db.apply_dedupe(&db.dedupe_snapshot()?, &[vec![a.clone(), b.clone()]])?;
    db.apply_dedupe(&db.dedupe_snapshot()?, &[vec![c.clone(), a.clone()]])?;
    assert_eq!(db.resolve_merchant_id(&b)?, c);
    assert_eq!(db.resolve_merchant_id(&a)?, c);
    db.put(&merchant("other", "Other", "https://other.example.com"))?;
    db.link("test", "a", "other")?;
    db.link("test", "b", "other")?;
    assert!(db.link("test", "c", "other").is_err());
    assert_eq!(db.resolve_source("test", "c")?, Some(c));
    Ok(())
}
#[tokio::test]
async fn provider_scan_dry_run_apply_and_errors() -> Result<()> {
    use std::io::{Read, Write};
    fn serve(answer: Value) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut response_answers = serde_json::Map::new();
            let mut request = Vec::new();
            let mut buf = [0; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap();
                request.extend_from_slice(&buf[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if request.len() >= end + 4 + length {
                        let body: Value =
                            serde_json::from_slice(&request[end + 4..end + 4 + length]).unwrap();
                        assert_eq!(body["questions"].as_object().unwrap().len(), 2);
                        for (key, question) in body["questions"].as_object().unwrap() {
                            assert_ne!(
                                question["instructions"]["left"]["merchant"]["name"],
                                question["instructions"]["right"]["merchant"]["name"]
                            );
                            response_answers.insert(key.clone(), answer.clone());
                        }
                        assert!(
                            body["questions"]
                                .as_object()
                                .unwrap()
                                .values()
                                .next()
                                .unwrap()["instructions"]["left"]["sources"]
                                .is_array()
                        );
                        break;
                    }
                }
                assert!(n > 0);
            }
            let body = json!({"answers": response_answers}).to_string();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        });
        (url, handle)
    }
    let db = MerchantStore::temporary()?;
    db.import(&[
        record("a", "Brand", "https://example.com"),
        record("b", "Brend", "https://www.example.com"),
        record("c", "Brand", "https://example.com"),
    ])?;
    assert!(
        run_at(
            db.clone(),
            Some("test".into()),
            "test".into(),
            0.98,
            false,
            0,
            "unused"
        )
        .await
        .is_err()
    );
    let (url, server) = serve(answer("same", 0.99, 0.99));
    let report = run_at(
        db.clone(),
        Some("test".into()),
        "test".into(),
        0.98,
        true,
        10,
        &url,
    )
    .await?;
    server.join().unwrap();
    assert_eq!(report.groups.len(), 1);
    assert_eq!(
        report
            .decisions
            .iter()
            .filter(|d| d.method == "rule")
            .count(),
        1
    );
    assert_eq!(
        report
            .decisions
            .iter()
            .filter(|d| d.method == "jev")
            .count(),
        2
    );
    assert!(report.run_id.is_none());
    assert_eq!(db.stats()?.total, 3);
    let (url, server) = serve(json!({"type":"choice","choice":"same"}));
    let invalid = run_at(
        db.clone(),
        Some("test".into()),
        "test".into(),
        0.98,
        false,
        10,
        &url,
    )
    .await?;
    assert_eq!(invalid.errors, 2);
    assert!(invalid.run_id.is_none());
    assert!(
        invalid
            .decisions
            .iter()
            .filter(|d| d.method == "jev")
            .all(|d| d.error.is_some() && !d.accepted)
    );
    server.join().unwrap();
    assert_eq!(db.stats()?.total, 3);
    let (url, server) = serve(answer("same", 0.99, 0.99));
    let report = run_at(
        db.clone(),
        Some("test".into()),
        "test".into(),
        0.98,
        false,
        10,
        &url,
    )
    .await?;
    server.join().unwrap();
    assert!(report.run_id.is_some());
    assert_eq!(db.stats()?.total, 1);
    assert_eq!(
        run_at(db, None, "test".into(), 0.98, false, 10, "unused")
            .await?
            .candidates,
        0
    );
    Ok(())
}
#[test]
fn stale_plan_changes_nothing() -> Result<()> {
    let db = MerchantStore::temporary()?;
    db.import(&[
        record("a", "Brand", "https://example.com"),
        record("b", "Brand", "https://example.com"),
    ])?;
    let before = db.dedupe_snapshot()?;
    let group = before.merchants.iter().map(|m| m.id.clone()).collect();
    db.put(&merchant("new", "New", "https://new.example.com"))?;
    assert!(db.apply_dedupe(&before, &[group]).is_err());
    assert_eq!(db.stats()?.total, 3);
    Ok(())
}
