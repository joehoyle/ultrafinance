use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
};

#[derive(Clone, Default)]
struct TraceCapture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
impl Write for TraceCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn proposal(outcome: &str) -> Proposal {
    serde_json::from_value(json!({"outcome":outcome,"merchant_id":null,"name":"Novel Cafe","website":"https://novel.test/","markets":["CA"],"explanation":"The business site supports the name and locality in the descriptor.","evidence":[{"url":"https://novel.test/","summary":"Business site identifies Novel Cafe in Canada."}]})).unwrap()
}

#[test]
fn catalog_payload_is_bounded_and_omits_source_records_and_unrelated_aliases() -> Result<()> {
    let mut merchant: Merchant = serde_json::from_value(
        json!({"id":"known","name":"Novel Cafe","website":"https://novel.test/","markets":["CA"],"logo_url":"https://novel.test/logo.png","sources":["private-source-metadata"]}),
    )?;
    merchant.aliases = (0..1000)
        .map(|i| format!("UNRELATED BANK ALIAS {i}"))
        .collect();
    merchant.aliases.extend([
        "SQ NOVEL CAFE".into(),
        "sq novel cafe".into(),
        "NOVEL CAFE #123".into(),
        "Novel Cafe".into(),
        format!("NOVEL {}", "oversized".repeat(100)),
    ]);
    let record = SourceRecord {
        source: "private-source-metadata".into(),
        external_id: "external-record".into(),
        merchant: merchant.clone(),
        attribution: "source-credit".into(),
        license: "license-data".into(),
        url: "https://source.test/".into(),
        version: Some("version-data".into()),
        raw: json!({"large_payload":"do-not-send".repeat(10000)}),
    };
    let candidate = Candidate {
        merchant,
        score: 0.99,
        exact: true,
        trusted: true,
        provenance: vec![record],
        interpretation_evidence: vec![],
        retrieval_evidence: vec![],
        business_evidence: serde_json::Value::Null,
        resolution_id: Some("private-resolution".into()),
        pending_import: false,
        regex_match_length: None,
    };
    let old_bytes = serde_json::to_vec(&candidate)?.len();
    let candidates = vec![candidate; 20];
    let compact = compact_candidates(&candidates, "SQ NOVEL CAFE");
    assert_eq!(compact.len(), MAX_CATALOG_CANDIDATES);
    assert_eq!(compact[0].aliases.len(), 2);
    assert_eq!(normalize(compact[0].aliases[0]), "sq novel cafe");
    let encoded = serde_json::to_string(&compact[0])?;
    let fields: Value = serde_json::from_str(&encoded)?;
    assert_eq!(fields["id"], "known");
    assert_eq!(fields["name"], "Novel Cafe");
    assert_eq!(fields["website"], "https://novel.test/");
    assert_eq!(fields["markets"], json!(["CA"]));
    assert_eq!(fields.as_object().unwrap().len(), 5);
    assert!(
        !encoded.contains("UNRELATED")
            && !encoded.contains("do-not-send")
            && !encoded.contains("private-source")
            && !encoded.contains("logo")
            && !encoded.contains("resolution")
    );
    assert!(encoded.len() < old_bytes / 100);
    let unrelated = compact_candidates(&candidates, "different merchant");
    assert!(unrelated[0].aliases.is_empty());
    let request: EnrichRequest = serde_json::from_value(
        json!({"description":"SQ NOVEL CAFE","country":"CA","amount":"99.00","extra":{"irrelevant":"private-extra"}}),
    )?;
    let payload = research_input(&request, &compact);
    assert_eq!(payload.as_object().unwrap().len(), 3);
    assert!(payload.get("interpretation").is_none() && payload.get("transaction").is_none());
    eprintln!(
        "Catalog fixture: {old_bytes} bytes per full candidate -> {} bytes per compact candidate",
        encoded.len()
    );
    Ok(())
}

#[tokio::test]
async fn local_enrichment_adds_compact_location_clues_without_provider_or_writes() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let merchant: Merchant = serde_json::from_value(
        json!({"id":"known","name":"Novel Cafe","website":"https://novel.test/"}),
    )?;
    store.put(&merchant)?;
    let before = serde_json::to_value((
        store.list(None, 100, 0)?,
        store.source_records("web-research", None, 100, 0)?,
        store.resolutions(None, 100)?,
    ))?;
    let request: EnrichRequest = serde_json::from_value(
        json!({"description":"SQ *NOVEL CAFE TORONTO ON CA","country":"CA"}),
    )?;
    let local = crate::enrich_locally(&request, store.clone()).await?;
    assert!(!local.candidates.is_empty());
    let summary = compact_local_enrichment(&local);
    assert_eq!(summary["processor_hint"], "Square");
    assert_eq!(summary["location"]["status"], "extracted");
    assert_eq!(summary["location"]["city"], "Toronto");
    assert_eq!(summary["location"]["country"], "CA");
    let hints = summary["tentative_interpretations"].as_array().unwrap();
    assert!(hints.len() <= 5);
    assert!(
        hints
            .iter()
            .any(|h| h["merchant_text"] == "NOVEL CAFE TORONTO ON CA")
    );
    assert!(hints.iter().any(|h| {
        h["location_hint"]["city"]
            .as_str()
            .is_some_and(|city| city.eq_ignore_ascii_case("Toronto"))
    }));
    let encoded = serde_json::to_string(&summary)?;
    assert!(!encoded.contains("geoname_ids") && !encoded.contains("provenance"));
    assert!(summary["location"].get("address").is_none());
    assert_eq!(
        before,
        serde_json::to_value((
            store.list(None, 100, 0)?,
            store.source_records("web-research", None, 100, 0)?,
            store.resolutions(None, 100)?,
        ))?
    );
    assert!(store.enrichment_logs(None, None, 10, 0)?.is_empty());
    assert!(store.resolutions(None, 10)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn same_name_results_preserve_bounded_geography_and_consulted_web_facts() -> Result<()> {
    let store = MerchantStore::temporary()?;
    for (id, website) in [
        ("granby", "https://granby.test/"),
        ("namesake", "https://other.test/"),
    ] {
        store.put(&serde_json::from_value(json!({"id":id,"name":"Centre Aquatique Desjardins","website":website,"markets":["CA"]}))?)?;
    }
    for (index, (id, city)) in [
        ("granby", "Toronto"),
        ("granby", "Bromont"),
        ("granby", "Granby"),
        ("granby", "Granby"),
        ("granby", "Montreal"),
        ("namesake", "Saint-Hyacinthe"),
    ]
    .into_iter()
    .enumerate()
    {
        store.import_locations(&[serde_json::from_value(json!({
            "source":"research-fixture","external_id":format!("location-{index}"),
            "merchant":{"merchant_id":id},
            "location":{"precision":"outlet","address":format!("{index} Example Road"),"city":city,"region":"QC","country":"CA"},
            "aliases":[],"attribution":"fixture-credit","license":"fixture-license","url":"https://fixture.test/"
        }))?])?;
    }
    let candidates = store.search("Centre Aquatique Desjardins Granby", Some("CA"), 10)?;
    let compact = catalog_candidates(
        &store,
        &candidates,
        "Centre Aquatique Desjardins Granby",
        Some("CA"),
    )?;
    let granby = compact.iter().find(|c| c.id == "granby").unwrap();
    assert_eq!(granby.locations.len(), 3);
    assert_eq!(granby.locations[0].city.as_deref(), Some("Granby"));
    let other = compact.iter().find(|c| c.id == "namesake").unwrap();
    assert_eq!(other.locations[0].city.as_deref(), Some("Saint-Hyacinthe"));
    let encoded = serde_json::to_string(&compact)?;
    for omitted in [
        "Example Road",
        "fixture-credit",
        "fixture-license",
        "external_id",
        "latitude",
        "provenance",
    ] {
        assert!(!encoded.contains(omitted));
    }
    let before = serde_json::to_value((
        store.list(None, 100, 0)?,
        store.source_records("web-research", None, 100, 0)?,
        store.resolutions(None, 100)?,
    ))?;
    let fact = "Official city page identifies Centre Aquatique Desjardins as a public pool in Granby, Quebec, Canada.";
    let (endpoint, server) = mock(vec![
        (
            200,
            json!({"status":"completed","output":[
                {"type":"web_search_call","status":"completed","action":{"type":"search","sources":[{"url":"https://granby.test/"}]}},
                call("search_merchants",json!({"query":"Centre Aquatique Desjardins Granby","web_evidence":[
                    {"url":"https://granby.test/#pool","summary":fact},
                    {"url":"https://unconsulted.test/","summary":"unsupported fact"}
                ]}))
            ]}),
        ),
        (
            200,
            json!({"status":"completed","output":[call("match_merchant",json!({"merchant_id":"granby","source_urls":["https://granby.test/"]}))]}),
        ),
    ]);
    let request =
        serde_json::from_value(json!({"description":"SQ GRANBY SWIMMING POOL","country":"CA"}))?;
    let report = investigate(
        &reqwest::Client::new(),
        &endpoint,
        "test-key",
        "test-model",
        &request,
        &store,
    )
    .await?;
    assert_eq!(report.merchant.unwrap().id, "granby");
    let requests = server.join().unwrap();
    assert_eq!(
        requests[0]["tools"][0]["user_location"],
        json!({"type":"approximate","country":"CA"})
    );
    let tool_output = requests[1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .unwrap();
    let reply: Value = serde_json::from_str(tool_output["output"].as_str().unwrap())?;
    let granby = reply["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "granby")
        .unwrap();
    assert_eq!(granby["locations"][0]["city"], "Granby");
    assert_eq!(
        reply["web_evidence"],
        json!([{"url":"https://granby.test/","summary":fact}])
    );
    assert_eq!(
        before,
        serde_json::to_value((
            store.list(None, 100, 0)?,
            store.source_records("web-research", None, 100, 0)?,
            store.resolutions(None, 100)?,
        ))?
    );
    assert!(store.enrichment_logs(None, None, 10, 0)?.is_empty());
    assert!(tools(true, None)[0].get("user_location").is_none());
    Ok(())
}

#[test]
fn fabricated_sources_ids_and_unchecked_names_are_rejected() {
    let sources = HashSet::from(["https://novel.test/".into()]);
    let searches = HashSet::from(["novel cafe".into()]);
    let ids = HashSet::from(["known".into()]);
    assert!(validate(&proposal("new"), &sources, &ids, &searches).is_ok());
    assert!(validate(&proposal("new"), &HashSet::new(), &ids, &searches).is_err());
    assert!(validate(&proposal("new"), &sources, &ids, &HashSet::new()).is_err());
    let mut p = proposal("existing");
    p.merchant_id = Some("invented".into());
    assert!(validate(&p, &sources, &ids, &searches).is_err());
    p.merchant_id = Some("known".into());
    assert!(validate(&p, &sources, &ids, &searches).is_ok());
    assert!(validate(&proposal("unresolved"), &sources, &ids, &searches).is_err());
    assert!(checked_url("https://user:secret@novel.test/").is_err());
    assert!(checked_url("file:///tmp/site").is_err());
}

#[test]
fn equivalent_page_urls_use_the_consulted_source_but_other_pages_are_rejected() {
    let sources = HashSet::from(["https://novel.test/".into()]);
    let searches = HashSet::from(["novel cafe".into()]);
    let mut p = proposal("new");
    p.website = Some("https://NOVEL.test:443".into());
    p.evidence[0].url = "https://novel.test/#about".into();
    let prepared = prepare_proposal(
        serde_json::to_value(&p).unwrap(),
        &sources,
        &HashSet::new(),
        &searches,
    )
    .unwrap();
    assert_eq!(prepared.website.as_deref(), Some("https://novel.test/"));
    assert_eq!(prepared.evidence[0].url, "https://novel.test/");
    for url in [
        "https://novel.test/contact",
        "https://novel.test/?different=1",
        "http://novel.test/",
        "https://other.test/",
    ] {
        p.evidence[0].url = url.into();
        assert!(
            prepare_proposal(
                serde_json::to_value(&p).unwrap(),
                &sources,
                &HashSet::new(),
                &searches
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn unsupported_citation_is_returned_to_the_model_for_correction() -> Result<()> {
    let store = MerchantStore::temporary()?;
    store.put(&serde_json::from_value(
        json!({"id":"known", "name":"Novel Cafe"}),
    )?)?;
    let before = serde_json::to_value((
        store.list(None, 100, 0)?,
        store.source_records("web-research", None, 100, 0)?,
        store.resolutions(None, 100)?,
    ))?;
    let mut p = proposal("existing");
    p.merchant_id = Some("known".into());
    p.evidence[0].url = "https://invented.test/".into();
    let mut corrected = proposal("existing");
    corrected.merchant_id = Some("known".into());
    corrected.evidence[0].url = "https://novel.test/#about".into();
    let (endpoint, server) = mock(vec![
        (
            200,
            json!({"status":"completed","output":[
                {"type":"web_search_call","id":"ws1","status":"completed","action":{"type":"search","sources":[{"url":"https://novel.test/"}]}},
                call("search_merchants", json!({"query":"Novel Cafe"}))
            ]}),
        ),
        (
            200,
            json!({"status":"completed","output":[terminal_call(p)]}),
        ),
        (
            200,
            json!({"status":"completed","output":[terminal_call(corrected)]}),
        ),
    ]);
    let request =
        serde_json::from_value(json!({"description":"SQ GRANBY SWIMMING POOL","country":"CA"}))?;
    let captured = TraceCapture::default();
    let mut session = ResearchSession {
        usage: Usage::default(),
        trace: Trace::with_writer(captured.clone(), "test-key"),
    };
    let mut report = investigate_with_usage(
        &reqwest::Client::new(),
        &endpoint,
        "test-key",
        "test-model",
        &request,
        &store,
        &mut session,
    )
    .await?;
    let requests = server.join().unwrap();
    let trace = String::from_utf8(captured.0.lock().unwrap().clone())?;
    assert!(trace.contains("step 1: LLM request"));
    assert!(trace.contains("step 1: LLM response"));
    assert!(trace.contains("local tool call"));
    assert!(trace.contains("local tool output"));
    assert!(trace.contains("tool validation feedback"));
    assert!(trace.contains("terminal tool result (local; not sent to LLM)"));
    assert!(trace.contains("search_merchants") && trace.contains("match_merchant"));
    assert!(trace.contains("SQ GRANBY SWIMMING POOL") && trace.contains("Novel Cafe"));
    assert!(!trace.contains("test-key") && !trace.contains("Authorization"));
    let feedback: Value = serde_json::from_str(
        requests[2]["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| {
                item["type"] == "function_call_output" && item["call_id"] == "call_match_merchant"
            })
            .unwrap()["output"]
            .as_str()
            .unwrap(),
    )?;
    let catalog_reply: Value = serde_json::from_str(
        requests[1]["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| {
                item["call_id"] == "call_search_merchants" && item["type"] == "function_call_output"
            })
            .unwrap()["output"]
            .as_str()
            .unwrap(),
    )?;
    assert_eq!(catalog_reply["candidates"][0]["id"], "known");
    assert!(catalog_reply["candidates"][0].get("merchant").is_none());
    assert!(catalog_reply["candidates"][0].get("provenance").is_none());
    assert_eq!(feedback["accepted"], false);
    assert_eq!(feedback["consulted_urls"], json!(["https://novel.test/"]));
    assert!(feedback["error"].as_str().unwrap().contains("URL absent"));
    assert!(matches!(report.proposal.outcome, Outcome::Existing));
    assert_eq!(report.proposal.evidence[0].url, "https://novel.test/");
    assert_eq!(report.merchant.as_ref().unwrap().id, "known");
    create(&mut report, &store).await?;
    assert!(!report.saved);
    assert_eq!(
        before,
        serde_json::to_value((
            store.list(None, 100, 0)?,
            store.source_records("web-research", None, 100, 0)?,
            store.resolutions(None, 100)?,
        ))?
    );
    Ok(())
}

#[test]
fn terminal_tools_reject_mixed_fields_and_results_contain_only_the_decision() -> Result<()> {
    assert!(
        tool_proposal(
            "match_merchant",
            json!({"merchant_id":"known","source_urls":[],"name":"unexpected"})
        )
        .is_err()
    );
    assert!(tool_proposal("propose_merchant", json!({"name":"Cafe","website":"https://novel.test/","markets":[],"source_urls":[],"merchant_id":"invented"})).is_err());
    assert!(tool_proposal("unresolved", json!({"merchant_id":"known"})).is_err());
    assert!(
        tool_proposal(
            "match_merchant",
            json!({"merchant_id":null,"source_urls":[]})
        )
        .is_err()
    );
    let mut report = Report {
        description: "SQ CAFE".into(),
        country: Some("CA".into()),
        model: "test-model".into(),
        proposal: proposal("new"),
        merchant: None,
        saved: false,
        verified: false,
        consulted_urls: vec![],
        usage: Usage::default(),
    };
    assert_eq!(
        serde_json::to_value(result(&report)?)?,
        json!({"status":"new_merchant","merchant":{"name":"Novel Cafe","website":"https://novel.test/","markets":["CA"]}})
    );
    report.proposal = tool_proposal(
        "match_merchant",
        json!({"merchant_id":"known","source_urls":["https://novel.test/"]}),
    )?;
    report.merchant = Some(serde_json::from_value(
        json!({"id":"known","name":"Novel Cafe"}),
    )?);
    assert_eq!(
        serde_json::to_value(result(&report)?)?,
        json!({"status":"matched","merchant_id":"known"})
    );
    report.saved = true;
    // Saving a descriptor alias for an existing identity is still a match.
    assert_eq!(
        serde_json::to_value(result(&report)?)?,
        json!({"status":"matched","merchant_id":"known"})
    );
    report.proposal = proposal("new");
    assert_eq!(
        serde_json::to_value(result(&report)?)?,
        json!({"status":"created","merchant_id":"known"})
    );
    report.saved = false;
    report.proposal = tool_proposal("unresolved", json!({}))?;
    report.merchant = None;
    assert_eq!(
        serde_json::to_value(result(&report)?)?,
        json!({"status":"unresolved"})
    );
    let schema = tools(true, None);
    for tool in schema
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["type"] == "function")
    {
        assert_eq!(tool["strict"], true);
        assert_eq!(tool["parameters"]["additionalProperties"], false);
        assert!(
            tool["parameters"]["properties"]
                .get("explanation")
                .is_none()
        );
        assert_ne!(tool["name"], "finish_research");
    }
    Ok(())
}

fn terminal_call(p: Proposal) -> Value {
    let sources: Vec<_> = p.evidence.into_iter().map(|e| e.url).collect();
    match p.outcome {
        Outcome::Existing => call(
            "match_merchant",
            json!({"merchant_id":p.merchant_id,"source_urls":sources}),
        ),
        Outcome::New => call(
            "propose_merchant",
            json!({"name":p.name,"website":p.website,"markets":p.markets,"source_urls":sources}),
        ),
        Outcome::Unresolved => call("unresolved", json!({})),
    }
}

fn call(name: &str, arguments: Value) -> Value {
    json!({"type":"function_call","id":format!("fc_{name}"),"call_id":format!("call_{name}"),"name":name,"arguments":arguments.to_string(),"status":"completed"})
}

/// Exercise the actual HTTP protocol without credentials or live web requests.
fn mock(responses: Vec<(u16, Value)>) -> (String, std::thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/responses", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let mut requests = vec![];
        for (status, response) in responses {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut bytes = vec![];
            let header_end = loop {
                let mut buf = [0; 4096];
                let count = socket.read(&mut buf).unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buf[..count]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|s| s.trim().parse().unwrap())
                })
                .unwrap();
            while bytes.len() < header_end + length {
                let mut buf = [0; 4096];
                let count = socket.read(&mut buf).unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buf[..count]);
            }
            requests.push(serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap());
            let body = response.to_string();
            write!(socket, "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
        requests
    });
    (endpoint, handle)
}

#[tokio::test]
async fn web_catalog_proposal_and_explicit_creation_preserve_identity_without_learning_aliases()
-> Result<()> {
    let store = MerchantStore::temporary()?;
    let before = serde_json::to_value((
        store.list(None, 100, 0)?,
        store.source_records("web-research", None, 100, 0)?,
        store.resolutions(None, 100)?,
    ))?;
    let p = proposal("new");
    let mut responses = vec![
        (
            200,
            json!({"status":"completed","output":[{"type":"reasoning","id":"r1","summary":[],"encrypted_content":"opaque"},{"type":"web_search_call","id":"ws1","status":"completed","action":{"type":"search","sources":[{"url":"https://novel.test/","type":"url"}]}}]}),
        ),
        (
            200,
            json!({"status":"completed","output":[call("search_merchants", json!({"query":"Novel Cafe"}))]}),
        ),
        (
            200,
            json!({"status":"completed","output":[terminal_call(p)]}),
        ),
    ];
    for (index, (_, response)) in responses.iter_mut().enumerate() {
        response["usage"] = json!({"input_tokens":1000 + index as u64 * 100,"input_tokens_details":{"cached_tokens":([0,400,500][index]),"cache_write_tokens":200},"output_tokens":([100,70,60][index]),"output_tokens_details":{"reasoning_tokens":([20,50,50][index])}});
    }
    let (endpoint, server) = mock(responses);
    let request: EnrichRequest =
        serde_json::from_value(json!({"description":"SQ *NOVEL CAFE","country":"CA"}))?;
    let mut report = investigate(
        &reqwest::Client::new(),
        &endpoint,
        "test-key",
        "gpt-6.1-sol",
        &request,
        &store,
    )
    .await?;
    assert_eq!(
        before,
        serde_json::to_value((
            store.list(None, 100, 0)?,
            store.source_records("web-research", None, 100, 0)?,
            store.resolutions(None, 100)?,
        ))?
    );
    assert!(!report.saved && !report.verified);
    assert_eq!(report.usage.responses, 3);
    assert_eq!(report.usage.input_tokens, 3300);
    assert_eq!(report.usage.output_tokens, 230);
    assert_eq!(report.usage.cached_input_tokens, 900);
    assert_eq!(report.usage.cache_write_tokens, 600);
    assert_eq!(report.usage.reasoning_tokens, 120);
    assert!((report.usage.estimated_cost_usd.unwrap() - 0.01749).abs() < 1e-9);
    let requests = server.join().unwrap();
    assert_eq!(requests[0]["store"], false);
    assert_eq!(requests[0]["service_tier"], "default");
    assert_eq!(requests[0]["tool_choice"]["type"], "web_search");
    assert_eq!(requests[0]["tools"][0]["search_context_size"], "low");
    for (index, request) in requests.iter().enumerate() {
        assert_eq!(request["max_tool_calls"], 1);
        let web_tools = request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|tool| tool["type"] == "web_search")
            .count();
        assert_eq!(web_tools, usize::from(index == 0));
    }
    assert!(
        requests[1]["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["encrypted_content"] == "opaque")
    );
    assert!(
        requests[2]["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["type"] == "function_call_output"
                && i["call_id"] == "call_search_merchants")
    );
    create(&mut report, &store).await?;
    let id = report.merchant.as_ref().unwrap().id.clone();
    assert!(report.saved);
    create(&mut report, &store).await?;
    assert!(report.saved);
    assert_eq!(report.merchant.as_ref().unwrap().id, id);
    assert_eq!(store.list(None, 10, 0)?.merchants.len(), 1);
    assert!(
        !report
            .merchant
            .as_ref()
            .unwrap()
            .aliases
            .iter()
            .any(|alias| normalize(alias) == normalize(&request.description))
    );
    assert!(store.resolutions(None, 10)?.is_empty());
    assert!(store.enrichment_logs(None, None, 10, 0)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn extra_web_calls_are_rejected_before_accepting_a_decision() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let before = serde_json::to_value((
        store.list(None, 100, 0)?,
        store.source_records("web-research", None, 100, 0)?,
        store.resolutions(None, 100)?,
    ))?;
    let request: EnrichRequest = serde_json::from_value(json!({"description":"Novel Cafe"}))?;
    let web = json!({"type":"web_search_call","status":"completed",
        "action":{"type":"search","sources":[{"url":"https://novel.test/"}]}});
    for later_round in [false, true] {
        let mut responses = vec![];
        if later_round {
            responses.push((200, json!({"status":"completed","output":[web.clone()]})));
        }
        let mut output = vec![web.clone()];
        if !later_round {
            output.push(web.clone());
        }
        output.push(call("unresolved", json!({})));
        responses.push((200, json!({"status":"completed","output":output})));
        let (endpoint, server) = mock(responses);
        let mut session = ResearchSession {
            usage: Usage::default(),
            trace: Trace::new(false, "test-key"),
        };
        let error = investigate_with_usage(
            &reqwest::Client::new(),
            &endpoint,
            "test-key",
            "test-model",
            &request,
            &store,
            &mut session,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("single web-search budget"));
        assert_eq!(session.usage.web_searches, 2);
        server.join().unwrap();
        assert_eq!(
            before,
            serde_json::to_value((
                store.list(None, 100, 0)?,
                store.source_records("web-research", None, 100, 0)?,
                store.resolutions(None, 100)?,
            ))?
        );
        assert!(store.enrichment_logs(None, None, 10, 0)?.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn existing_unresolved_and_step_limit_never_write_catalog() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let merchant: Merchant = serde_json::from_value(
        json!({"id":"known", "name":"Novel Cafe", "website":"https://novel.test/"}),
    )?;
    store.put(&merchant)?;
    let before = serde_json::to_value((
        store.list(None, 100, 0)?,
        store.source_records("web-research", None, 100, 0)?,
        store.resolutions(None, 100)?,
    ))?;
    let request: EnrichRequest = serde_json::from_value(json!({"description":"Novel Cafe"}))?;
    for outcome in ["existing", "unresolved"] {
        let mut p = proposal(outcome);
        if outcome == "existing" {
            p.merchant_id = Some("known".into());
        } else {
            // Reproduce an abstention containing a tentative catalog ID and
            // business fields, as providers can return despite instructions.
            p.merchant_id = Some("known".into());
        }
        let (endpoint, server) = mock(vec![(
            200,
            json!({"status":"completed","output":[
                {"type":"web_search_call","id":"ws1","status":"completed","action":{"type":"search","sources":[{"url":"https://novel.test/"}]}},
                terminal_call(p)
            ]}),
        )]);
        let mut report = investigate(
            &reqwest::Client::new(),
            &endpoint,
            "test-key",
            "test-model",
            &request,
            &store,
        )
        .await?;
        let requests = server.join().unwrap();
        let payload: Value =
            serde_json::from_str(requests[0]["input"][0]["content"].as_str().unwrap())?;
        assert_eq!(payload["description"], "Novel Cafe");
        assert_eq!(payload["catalog_candidates"][0]["id"], "known");
        assert_eq!(payload["local_enrichment"]["merchant"]["status"], "matched");
        assert_eq!(
            payload["local_enrichment"]["merchant"]["merchant_id"],
            "known"
        );
        assert!(payload.get("interpretation").is_none());
        assert!(payload["catalog_candidates"][0].get("provenance").is_none());
        create(&mut report, &store).await?;
        assert!(!report.saved);
        assert_eq!(report.merchant.is_some(), outcome == "existing");
        if outcome == "unresolved" {
            assert!(report.proposal.merchant_id.is_none());
            assert!(report.proposal.name.is_none());
            assert!(report.proposal.website.is_none());
            assert!(report.proposal.markets.is_empty());
            assert!(report.proposal.evidence.is_empty());
            assert!(report.proposal.explanation.contains("descriptor"));
        }
        assert_eq!(
            before,
            serde_json::to_value((
                store.list(None, 100, 0)?,
                store.source_records("web-research", None, 100, 0)?,
                store.resolutions(None, 100)?,
            ))?
        );
    }
    let (endpoint, server) = mock(vec![
        (200, json!({"status":"completed", "output":[]}));
        MAX_ROUNDS
    ]);
    let error = investigate(
        &reqwest::Client::new(),
        &endpoint,
        "test-key",
        "test-model",
        &request,
        &store,
    )
    .await
    .unwrap_err();
    server.join().unwrap();
    assert!(error.to_string().contains("exceeded 6 steps"));
    assert_eq!(
        before,
        serde_json::to_value((
            store.list(None, 100, 0)?,
            store.source_records("web-research", None, 100, 0)?,
            store.resolutions(None, 100)?,
        ))?
    );
    Ok(())
}

#[tokio::test]
async fn creation_reconciles_with_an_existing_catalog_identity() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let merchant: Merchant = serde_json::from_value(
        json!({"id":"known", "name":"Novel Cafe", "website":"https://novel.test/"}),
    )?;
    store.put(&merchant)?;
    let mut report = Report {
        description: "SQ *NOVEL CAFE".into(),
        country: Some("CA".into()),
        model: "test-model".into(),
        proposal: proposal("new"),
        merchant: None,
        saved: false,
        verified: false,
        consulted_urls: vec!["https://novel.test/".into()],
        usage: Usage::default(),
    };
    create(&mut report, &store).await?;
    assert_eq!(report.merchant.as_ref().unwrap().id, "known");
    assert_eq!(store.list(None, 10, 0)?.merchants.len(), 1);
    Ok(())
}

#[tokio::test]
async fn provider_errors_do_not_echo_credentials_or_response_content() {
    let (endpoint, server) = mock(vec![(401, json!({"error":"sensitive echoed request"}))]);
    let error = post(&reqwest::Client::new(), &endpoint, "secret", &json!({}))
        .await
        .unwrap_err()
        .to_string();
    server.join().unwrap();
    assert!(error.contains("401") && error.contains("OPENAI_API_KEY"));
    assert!(!error.contains("secret") && !error.contains("sensitive"));
}

fn fallback_engine(store: MerchantStore, endpoint: &str) -> crate::Enricher {
    let mut research = Research::new("fixture-key".into(), "fixture-model".into()).unwrap();
    research.endpoint = endpoint.into();
    crate::Enricher::with_store(None, "unused".into(), 0.95, store)
        .unwrap()
        .with_research(Some(research))
}
fn web_search_then(name: &str, terminal: Value) -> Vec<(u16, Value)> {
    vec![
        (
            200,
            json!({"status":"completed","output":[
                {"type":"web_search_call","status":"completed","action":{"type":"search","sources":[{"url":"https://novel.test/"}]}},
                call("search_merchants", json!({"query":name,"web_evidence":[]}))
            ]}),
        ),
        (200, json!({"status":"completed","output":[terminal]})),
    ]
}
#[tokio::test]
async fn fallback_is_opt_in_and_skips_resolved_invalid_and_evaluation_requests() -> Result<()> {
    let store = MerchantStore::temporary()?;
    store.put(&serde_json::from_value(
        json!({"id":"known","name":"Novel Cafe"}),
    )?)?;
    let enricher = fallback_engine(store.clone(), "http://127.0.0.1:1/not-called");
    let off = serde_json::from_value(json!({"description":"UNIDENTIFIED COUNTERPARTY ZZZ"}))?;
    assert!(matches!(
        enricher.enrich(&off).await?.merchant,
        crate::MerchantResult::Unresolved { .. }
    ));
    let matched = serde_json::from_value(json!({"description":"Novel Cafe","research":true}))?;
    assert!(matches!(
        enricher.enrich(&matched).await?.merchant,
        crate::MerchantResult::Matched { .. }
    ));
    let invalid = serde_json::from_value(json!({"description":"","research":true}))?;
    assert!(enricher.enrich(&invalid).await.is_err());
    let evaluator = crate::Enricher::for_evaluation(None, "unused".into(), 0.95, store.clone())?;
    let request = serde_json::from_value(
        json!({"description":"UNIDENTIFIED COUNTERPARTY ZZZ","research":true}),
    )?;
    assert!(matches!(
        evaluator.enrich(&request).await?.merchant,
        crate::MerchantResult::Unresolved { .. }
    ));
    assert!(
        store
            .source_records("web-research-alias", None, 10, 0)?
            .is_empty()
    );
    Ok(())
}
#[tokio::test]
async fn fallback_existing_match_saves_unverified_alias_and_full_audit_evidence() -> Result<()> {
    let store = MerchantStore::temporary()?;
    store.put(&serde_json::from_value(json!({"id":"known","name":"Novel Cafe","website":"https://novel.test/","logo_url":"https://novel.test/manual.png","aliases":["MANUAL NOVEL"]}))?)?;
    let (endpoint, server) = mock(web_search_then(
        "Novel Cafe",
        call(
            "match_merchant",
            json!({"merchant_id":"known","source_urls":["https://novel.test/"]}),
        ),
    ));
    let enricher = fallback_engine(store.clone(), &endpoint);
    let request: EnrichRequest = serde_json::from_value(
        json!({"description":"BANK-76359 COUNTERPARTY","country":"CA","research":true}),
    )?;
    let (response, details) = enricher.enrich_with_details(&request).await;
    let response = response?;
    assert_eq!(server.join().unwrap().len(), 2);
    let crate::MerchantResult::Matched { data } = response.merchant else {
        panic!("research did not match");
    };
    assert_eq!(data.id, "known");
    assert!(data.aliases.contains(&request.description));
    assert!(data.aliases.contains(&"MANUAL NOVEL".into()));
    assert_eq!(
        data.logo_url.as_deref(),
        Some("https://novel.test/manual.png")
    );
    assert!(
        response
            .attributions
            .iter()
            .any(|credit| credit.contains("https://novel.test/"))
    );
    assert_eq!(details["method"], "research");
    assert_eq!(details["research"]["saved"], true);
    assert!(details["research_attempted"].as_bool().unwrap());
    let mappings = store.resolutions(None, 10)?;
    assert_eq!(mappings.len(), 1);
    assert!(!mappings[0].verified);
    assert!(
        mappings[0]
            .evidence
            .as_deref()
            .unwrap()
            .contains("https://novel.test/")
    );
    assert!(!store.search(&request.description, Some("CA"), 10)?[0].trusted);
    assert_eq!(store.list(None, 10, 0)?.total, 1);
    assert_eq!(
        store
            .source_records("web-research-alias", None, 10, 0)?
            .len(),
        1
    );
    Ok(())
}
#[tokio::test]
async fn fallback_new_match_creates_merchant_with_descriptor_alias() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let asset = crate::logos::ImageAsset {
        id: "c".repeat(64),
        url: "https://novel.test/logo.webp".into(),
        mime_type: "image/webp".into(),
        width: 256,
        height: 256,
        byte_length: 100,
    };
    store.save_website_logo(&crate::logos::WebsiteLogo {
        website_url: "https://novel.test/".into(),
        source_url: "https://novel.test/logo.png".into(),
        kind: crate::logos::LogoKind::Logo,
        display: asset,
        checked_at: None,
    })?;
    let (endpoint, server) = mock(web_search_then(
        "Novel Cafe",
        call(
            "propose_merchant",
            json!({"name":"Novel Cafe","website":"https://novel.test/","markets":["CA"],"source_urls":["https://novel.test/"]}),
        ),
    ));
    let enricher = fallback_engine(store.clone(), &endpoint);
    let request: EnrichRequest = serde_json::from_value(
        json!({"description":"BANK-76359 COUNTERPARTY","country":"CA","research":true}),
    )?;
    let response = enricher.enrich(&request).await?;
    server.join().unwrap();
    let crate::MerchantResult::Matched { data } = response.merchant else {
        panic!("research did not match");
    };
    assert_eq!(data.name, "Novel Cafe");
    assert_eq!(
        data.logo_url.as_deref(),
        Some("https://novel.test/logo.webp")
    );
    assert!(data.aliases.contains(&request.description));
    assert_eq!(
        store
            .resolve_source("web-research", "https://novel.test/")?
            .as_deref(),
        Some(data.id.as_str())
    );
    assert_eq!(store.list(None, 10, 0)?.total, 1);
    assert_eq!(
        store
            .source_records("web-research-alias", None, 10, 0)?
            .len(),
        1
    );
    Ok(())
}
#[tokio::test]
async fn fallback_unresolved_and_provider_errors_do_not_save_catalog_data() -> Result<()> {
    for failed in [false, true] {
        let store = MerchantStore::temporary()?;
        let responses = if failed {
            vec![(401, json!({"error":"fixture-key must never leak"}))]
        } else {
            vec![(
                200,
                json!({"status":"completed","output":[{"type":"web_search_call","status":"completed","action":{"type":"search","sources":[]}},call("unresolved",json!({}))]}),
            )]
        };
        let (endpoint, server) = mock(responses);
        let enricher = fallback_engine(store.clone(), &endpoint);
        let request = serde_json::from_value(
            json!({"description":"UNIDENTIFIED COUNTERPARTY ZZZ","research":true}),
        )?;
        let result = enricher.enrich(&request).await;
        server.join().unwrap();
        if failed {
            let error = format!("{:#}", result.unwrap_err());
            assert!(!error.contains("fixture-key"));
        } else {
            assert!(matches!(
                result?.merchant,
                crate::MerchantResult::Unresolved { .. }
            ));
        }
        assert_eq!(store.list(None, 10, 0)?.total, 0);
        assert!(store.resolutions(None, 10)?.is_empty());
        assert!(
            store
                .source_records("web-research-alias", None, 10, 0)?
                .is_empty()
        );
    }
    Ok(())
}
#[test]
fn research_persistence_is_idempotent_and_conflicts_roll_back_new_merchants() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let request: EnrichRequest =
        serde_json::from_value(json!({"description":"BANK-76359 COUNTERPARTY","country":"CA"}))?;
    let report = || Report {
        description: request.description.clone(),
        country: request.country.clone(),
        model: "fixture-model".into(),
        proposal: proposal("new"),
        merchant: None,
        saved: false,
        verified: false,
        consulted_urls: vec!["https://novel.test/".into()],
        usage: Usage::default(),
    };
    let mut first = report();
    let merchant = first.persist(&request, &store)?.unwrap();
    let mut verified = store.resolutions(None, 10)?.remove(0);
    verified.verified = true;
    verified.evidence = Some("Explicitly reviewed descriptor identity".into());
    store.save_resolution(&verified)?;
    let mut again = report();
    assert_eq!(again.persist(&request, &store)?.unwrap().id, merchant.id);
    assert_eq!(store.list(None, 10, 0)?.total, 1);
    assert_eq!(
        store
            .source_records("web-research-alias", None, 10, 0)?
            .len(),
        1
    );
    let mut conflicting = report();
    conflicting.proposal.website = Some("https://different.test/".into());
    conflicting.proposal.name = Some("Entirely Different Business".into());
    assert!(conflicting.persist(&request, &store).is_err());
    assert_eq!(store.list(None, 10, 0)?.total, 1);
    assert!(
        store
            .resolve_source("web-research", "https://different.test/")?
            .is_none()
    );
    assert_eq!(store.resolutions(None, 10)?[0].merchant.id, merchant.id);
    assert!(store.resolutions(None, 10)?[0].verified);
    let mut next_charge = request.clone();
    next_charge.amount = Some("99.00".into());
    report().persist(&next_charge, &store)?;
    assert_eq!(
        store
            .source_records("web-research-alias", None, 10, 0)?
            .len(),
        1
    );
    assert_eq!(store.resolutions(None, 10)?.len(), 2);
    Ok(())
}
#[tokio::test]
async fn research_timeout_does_not_save_partial_identity_or_alias() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let mut research = Research::new("fixture-key".into(), "fixture-model".into())?;
    research.endpoint = format!("http://{}/responses", listener.local_addr()?);
    let request = serde_json::from_value(
        json!({"description":"UNIDENTIFIED COUNTERPARTY ZZZ","research":true}),
    )?;
    let error = research
        .investigate(&request, &store, false, false, Duration::from_millis(20))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("deadline"));
    drop(listener);
    assert_eq!(store.list(None, 10, 0)?.total, 0);
    assert!(store.resolutions(None, 10)?.is_empty());
    Ok(())
}
