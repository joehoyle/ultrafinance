use super::*;
pub(super) fn regex_search_scenarios(store: &MerchantStore) -> Result<()> {
    let rule = |id: &str, pattern: &str| {
        let mut r = record(id, &format!("Rule merchant {id}"));
        r.source = "open-enrichment".into();
        r.raw = serde_json::json!({"transaction_text_regexp":pattern, "parent_id":""});
        r
    };
    let broad = rule("regex-broad", r"(?i)^ZXQ(?: |\s)M");
    let specific = rule("regex-specific", r"(?i)^ZXQ ME UP");
    store.import(&[
        broad.clone(),
        specific.clone(),
        rule("regex-invalid", "["),
        rule("regex-empty", ".*"),
    ])?;
    let specific_id = store
        .resolve_source("open-enrichment", "regex-specific")?
        .unwrap();
    let broad_id = store
        .resolve_source("open-enrichment", "regex-broad")?
        .unwrap();
    let hits = store.search("sq * ZXQ ME UP 9876", Some("CA"), 10)?;
    assert_eq!(hits[0].merchant.id, specific_id);
    assert_eq!(hits[0].regex_match_length, Some(9));
    assert_eq!(hits[1].merchant.id, broad_id);
    assert_eq!(hits[1].regex_match_length, Some(5));
    assert!(!hits[0].exact && !hits[0].trusted);
    assert_eq!(hits[0].provenance[0].external_id, "regex-specific");
    assert!(
        store
            .search("ZXQ ME UP 9876", Some("US"), 10)?
            .iter()
            .any(|c| c.regex_match_length.is_some())
    );
    let tied = rule("regex-tied", r"(?i)^ZXQ ME UP");
    store.import(&[tied])?;
    assert_eq!(store.search("ZXQ ME UP 9876", None, 1)?.len(), 2);
    let mut refreshed = specific.clone();
    refreshed.raw["transaction_text_regexp"] = serde_json::json!(r"(?i)^REPLACEMENT\b");
    store.import(&[refreshed])?;
    assert!(
        !store
            .search("ZXQ ME UP 9876", None, 10)?
            .iter()
            .any(|c| c.merchant.id == specific_id && c.regex_match_length.is_some())
    );
    assert_eq!(
        store.search("REPLACEMENT 9876", None, 10)?[0].merchant.id,
        specific_id
    );
    store.link("open-enrichment", "regex-broad", &specific_id)?;
    assert_eq!(
        store.search("ZXQ MARATHON 9876", None, 10)?[0].merchant.id,
        specific_id
    );
    let mut negative = rule("regex-negative", r"^EXCLUDED\b");
    negative.raw["negativeAliases"] = serde_json::json!(["EXCLUDED TEST"]);
    store.import(&[negative])?;
    assert!(
        store
            .search("EXCLUDED TEST", None, 10)?
            .iter()
            .all(|c| c.regex_match_length.is_none())
    );
    let mut child = rule("regex-child", "CHILDRULE");
    child.raw["parent_id"] = serde_json::json!("some-parent");
    store.import(&[child])?;
    assert!(
        store
            .search("CHILDRULE", None, 10)?
            .iter()
            .all(|c| c.regex_match_length.is_none())
    );
    let mut manual = merchant("regex-manual", "Manually verified", "CA");
    manual.aliases = vec!["ZXQ ME UP 9876".into()];
    store.put(&manual)?;
    let hits = store.search("ZXQ ME UP 9876", None, 10)?;
    assert_eq!(hits[0].merchant.id, manual.id);
    assert!(hits[0].trusted && hits[0].exact);
    Ok(())
}
#[test]
fn open_enrichment_regex_search() -> Result<()> {
    regex_search_scenarios(&MerchantStore::temporary()?)
}
fn merchant(id: &str, name: &str, country: &str) -> Merchant {
    Merchant {
        counterparty_type: Default::default(),
        id: id.into(),
        name: name.into(),
        markets: vec![country.into()],

        website: None,
        logo_url: None,
        logo_source: None,
        aliases: vec![],
        sources: vec![],
    }
}
#[test]
fn interpretation_uses_catalog_city_without_discarding_full_name() -> Result<()> {
    let store = MerchantStore::temporary()?;
    store.put(&merchant("short", "Julius Cafe", "CA"))?;
    store.put(&merchant("full", "Julius Cafe Bromont", "CA"))?;
    let request: crate::EnrichRequest = serde_json::from_value(serde_json::json!({
        "description":"SQ* JULIUS CAFE BROMONT 00482", "country":"CA"
    }))?;
    let candidates = store.search_request(&request, 10)?;
    assert_eq!(candidates[0].merchant.id, "full");
    let short = candidates
        .iter()
        .find(|c| c.merchant.id == "short")
        .unwrap();
    assert!(
        short
            .interpretation_evidence
            .iter()
            .any(|e| e.possible_location.as_deref() == Some("BROMONT") && e.outlet.is_none())
    );
    let outlet: LocationRecord = serde_json::from_value(serde_json::json!({
        "source":"reviewed-outlets", "external_id":"julius-bromont", "aliases":["Julius Cafe Bromont"],
        "merchant":{"merchant_id":"short"},
        "location":{"id":null,"precision":"outlet", "address":"123 Test Street",
            "city":"Bromont","region":"QC","postal_code":null,"country":"CA","store_number":null},
        "attribution":"Test fixture", "license":"test", "url":"https://example.com/outlet"
    }))?;
    store.import_locations(&[outlet])?;
    let candidates = store.search_request(&request, 10)?;
    assert_eq!(candidates[0].merchant.id, "full");
    let short = candidates
        .iter()
        .find(|c| c.merchant.id == "short")
        .unwrap();
    assert!(!short.exact && !short.trusted);
    let evidence = short
        .interpretation_evidence
        .iter()
        .find_map(|e| e.outlet.as_ref())
        .unwrap();
    assert_eq!(evidence.city, "Bromont");
    assert_eq!(evidence.external_id, "julius-bromont");
    let mut other_country = request.clone();
    other_country.country = Some("US".into());
    assert!(
        store
            .search_request(&other_country, 10)?
            .iter()
            .flat_map(|c| &c.interpretation_evidence)
            .all(|e| e.outlet.is_none())
    );
    assert!(store.get("short")?.unwrap().aliases.is_empty());
    Ok(())
}
#[test]
fn fuzzy_names_use_source_linked_city_evidence_without_becoming_trusted() -> Result<()> {
    let store = MerchantStore::temporary()?;
    let csv = "fsq_place_id,name,country,address,locality,region,latitude,longitude,fsq_category_ids,date_closed\njulius,Julius Cafe,CA,35 John-Savage Rue,Bromont,QC,45.3,-72.6,\"[\"\"restaurant\"\"]\",\npizza,Julius Pizza,CA,1 Main St,Toronto,ON,43.6,-79.4,\"[\"\"restaurant\"\"]\",\nhotel,Hotel Bromont,CA,2 Main St,Bromont,QC,45.3,-72.6,\"[\"\"hotel\"\"]\",\n";
    let records = crate::foursquare::prepare(csv, "ca")?.0;
    store.import(&records)?;
    let request: crate::EnrichRequest = serde_json::from_value(serde_json::json!({
        "description":"SQ* JULIUS BROMONT"
    }))?;
    let candidates = store.search_request(&request, 10)?;
    assert_eq!(candidates[0].merchant.name, "Julius Cafe");
    let cafe = &candidates[0];
    assert!(!cafe.exact && !cafe.trusted);
    let support = cafe
        .interpretation_evidence
        .iter()
        .find(|e| e.merchant_text == "JULIUS")
        .unwrap();
    assert!(!support.name_exact);
    assert_eq!(support.matched_name, "Julius Cafe");
    assert_eq!(support.outlet.as_ref().unwrap().city, "Bromont");
    assert!(
        candidates
            .iter()
            .find(|c| c.merchant.name == "Julius Pizza")
            .unwrap()
            .interpretation_evidence
            .is_empty()
    );
    assert!(
        candidates
            .iter()
            .find(|c| c.merchant.name == "Hotel Bromont")
            .unwrap()
            .interpretation_evidence
            .is_empty()
    );
    let typo = serde_json::from_value(serde_json::json!({"description":"SQ* JULUIS BROMONT"}))?;
    let typo_support = crate::interpretation::catalog_support(
        &crate::interpretation::interpret(&typo),
        &cafe.merchant,
        &store.cached_locations(&cafe.merchant.id)?,
        None,
    );
    assert!(
        typo_support
            .iter()
            .any(|e| !e.name_exact && e.outlet.is_some())
    );
    for body in [
        serde_json::json!({"description":"SQ* JULIUS BROMONT", "country":"US"}),
        serde_json::json!({"description":"SQ* JULIUS MONTREAL"}),
        serde_json::json!({"description":"SQ* JULIUS"}),
    ] {
        let request = serde_json::from_value(body)?;
        assert!(
            store
                .search_request(&request, 10)?
                .iter()
                .all(|c| c.interpretation_evidence.is_empty())
        );
    }
    assert_eq!(
        store.get(&cafe.merchant.id)?.unwrap().aliases,
        cafe.merchant.aliases
    );
    assert!(
        !cafe
            .merchant
            .aliases
            .iter()
            .any(|a| normalize(a) == "julius")
    );
    Ok(())
}
#[test]
fn bank_format_retrieval_is_ranked_but_never_trusted() -> Result<()> {
    let store = MerchantStore::temporary()?;
    store.put(&merchant("carvana", "Carvana", "US"))?;
    store.put(&merchant("paypal", "PayPal", "US"))?;
    store.put(&merchant("recipient", "Recipient", "US"))?;
    for description in [
        "PAYPAL *CARVANA 402-935-7733 AZ",
        "10/01 POS PURCHASE PP *CARVANA REF: ABC123 2026-10-01",
        "Orig CO Name:CARVANA Orig ID:123456 Desc Date:261001 CO Entry Descr:PAYMENT Sec:CCD Trace#:123456789 Ind Name:RECIPIENT",
        "CARVANA DES:PAYMENT ID:123456 INDN:RECIPIENT CO ID:987654 PPD",
    ] {
        let request: crate::EnrichRequest = serde_json::from_value(serde_json::json!({
            "description":description, "country":"US"
        }))?;
        let candidates = store.search_request(&request, 1)?;
        assert_eq!(candidates[0].merchant.id, "carvana", "{description}");
        assert!(
            !candidates[0].exact && !candidates[0].trusted,
            "{description}"
        );
        assert!(
            candidates[0]
                .interpretation_evidence
                .iter()
                .any(|e| e.matched_name == "Carvana")
        );
    }
    assert!(store.get("carvana")?.unwrap().aliases.is_empty());
    Ok(())
}
#[test]
fn structured_location_hypotheses_retrieve_the_merchant_without_learning_aliases() -> Result<()> {
    let store = MerchantStore::temporary()?;
    store.put(&merchant("cafe", "Julius Cafe", "CA"))?;
    for description in [
        "SQ *JULIUS CAFE STORE 00482 TORONTO ON CAN",
        "JULIUS CAFE SAN FRANCISCO CA",
        "JULIUS CAFE / Amsterdam / NLD",
    ] {
        let request = serde_json::from_value(serde_json::json!({"description":description}))?;
        let candidates = store.search_request(&request, 1)?;
        assert_eq!(candidates[0].merchant.id, "cafe", "{description}");
        assert!(!candidates[0].exact && !candidates[0].trusted);
        assert!(
            candidates[0]
                .interpretation_evidence
                .iter()
                .any(|e| e.location_hint.is_some())
        );
    }
    assert!(store.get("cafe")?.unwrap().aliases.is_empty());
    Ok(())
}

#[test]
fn inferred_country_ranks_merchants_before_shortlist_truncation() -> Result<()> {
    let store = MerchantStore::temporary()?;
    for index in 0..12 {
        store.put(&merchant(
            &format!("a-us-{index:02}"),
            "Northstar Books",
            "US",
        ))?;
    }
    store.put(&merchant("z-ca", "Northstar Books", "CA"))?;
    let mut request: crate::EnrichRequest = serde_json::from_value(serde_json::json!({
        "description":"NORTHSTAR BOOKS TORONTO ON"
    }))?;
    let candidates = store.search_request(&request, 1)?;
    assert_eq!(candidates[0].merchant.id, "z-ca");
    assert!(!candidates[0].exact && !candidates[0].trusted);
    assert!(
        candidates[0]
            .interpretation_evidence
            .iter()
            .all(|e| e.outlet.is_none())
    );
    let all = store.search_request(&request, 20)?;
    assert_eq!(all.len(), 13);
    assert!(all.iter().any(|c| c.merchant.markets == ["US"]));
    assert!(request.country.is_none());
    assert!(store.get("z-ca")?.unwrap().aliases.is_empty());

    request.country = Some("US".into());
    assert_eq!(
        store.search_request(&request, 1)?[0].merchant.markets,
        ["US"]
    );
    request.country = None;
    request.location = Some(serde_json::from_value(serde_json::json!({"country":"US"}))?);
    assert_eq!(
        store.search_request(&request, 1)?[0].merchant.markets,
        ["US"]
    );
    Ok(())
}

#[test]
fn conflicting_inferred_countries_do_not_choose_a_ranking_country() -> Result<()> {
    let request = serde_json::from_value(serde_json::json!({
        "description":"NORTHSTAR BOOKS TORONTO ON"
    }))?;
    let mut interpretation = crate::interpretation::interpret(&request);
    assert_eq!(ranking_country(&request, &interpretation), Some("CA"));
    let mut conflicting = interpretation
        .hypotheses
        .iter()
        .find(|h| h.location_hint.is_some())
        .unwrap()
        .clone();
    conflicting.location_hint.as_mut().unwrap().country = Some("US".into());
    interpretation.hypotheses.push(conflicting);
    assert_eq!(ranking_country(&request, &interpretation), None);
    interpretation.hypotheses.clear();
    assert_eq!(ranking_country(&request, &interpretation), None);
    Ok(())
}

fn record(id: &str, name: &str) -> SourceRecord {
    SourceRecord {
        source: "test".into(),
        external_id: id.into(),
        merchant: merchant(id, name, "CA"),
        attribution: "Test data".into(),
        license: "test".into(),
        url: "https://example.com".into(),
        version: Some("1".into()),
        raw: serde_json::json!({}),
    }
}
#[test]
fn ascii_normalization_matches_unicode_reference() {
    fn reference(value: &str) -> String {
        value
            .nfkd()
            .filter(|c| !is_combining_mark(*c))
            .flat_map(char::to_lowercase)
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
    let ascii: String = (0..=127).map(char::from).collect();
    for value in [
        &ascii,
        "  A--B___123 \tZ!",
        "",
        "---",
        "Julius Café",
        "İß 東京",
    ] {
        assert_eq!(normalize(value), reference(value));
    }
}
#[test]
fn imports_are_untrusted_idempotent_and_preserve_manual_overrides() {
    let db = MerchantStore::temporary().unwrap();
    let mut imported = record("external-1", "Julius Cafe");
    imported.merchant.aliases = vec!["BANK ORIGINAL".into()];
    db.import(&[imported.clone()]).unwrap();
    let first = db.search("BANK ORIGINAL", Some("CA"), 10).unwrap();
    assert!(first[0].exact);
    assert!(!first[0].trusted);
    let id = first[0].merchant.id.clone();
    db.import(&[imported.clone()]).unwrap();
    let second = db.search("BANK ORIGINAL", Some("CA"), 10).unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].merchant.id, id);
    let mut manual = merchant(&id, "Julius Café corrected", "CA");
    manual.aliases = vec!["VERIFIED ALIAS".into()];
    db.put(&manual).unwrap();
    imported.merchant.name = "Bad refresh name".into();
    imported.merchant.aliases = vec!["NEW IMPORTED ALIAS".into()];
    db.import(&[imported]).unwrap();
    let refreshed = db.search("NEW IMPORTED ALIAS", Some("CA"), 10).unwrap();
    assert_eq!(refreshed[0].merchant.name, "Julius Café corrected");
    assert!(!refreshed[0].trusted);
    assert!(db.search("VERIFIED ALIAS", Some("CA"), 10).unwrap()[0].trusted);
    assert!(
        !db.search("BANK ORIGINAL", Some("CA"), 10)
            .unwrap()
            .iter()
            .any(|c| c.exact)
    );
}
#[test]
fn delta_imports_skip_unchanged_rows_and_retain_absent_records() {
    let db = MerchantStore::temporary().unwrap();
    let a = record("a", "Alpha");
    let mut b = record("b", "Beta");
    let first = db.import_delta(&[a.clone(), b.clone()]).unwrap();
    assert_eq!((first.added, first.updated, first.unchanged), (2, 0, 0));
    let id = db.resolve_source(&a.source, "a").unwrap().unwrap();
    let mut a = a;
    a.version = Some("new-whole-file-version".into());
    b.raw = serde_json::json!({"countryHints":["CA"],"discarded":true});
    let second = db.import_delta(&[a.clone(), b]).unwrap();
    assert_eq!((second.added, second.updated, second.unchanged), (0, 1, 1));
    assert_eq!(db.resolve_source(&a.source, "a").unwrap().unwrap(), id);
    let third = db.import_delta(&[a.clone()]).unwrap();
    assert_eq!(third.unchanged, 1);
    let rows = db.source_records(&a.source, None, 10, 0).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[1]["record"]["raw"],
        serde_json::json!({"countryHints":["CA"]})
    );
    assert_eq!(
        db.source_records(&a.source, Some("b"), 1, 0).unwrap().len(),
        1
    );
    assert!(
        db.source_records(&a.source, None, 10, 2)
            .unwrap()
            .is_empty()
    );
}
#[test]
fn source_links_survive_refresh_and_import_failure_is_atomic() {
    let db = MerchantStore::temporary().unwrap();
    let a = record("external-a", "Alpha merchant");
    let b = record("external-b", "Beta merchant");
    db.import(&[a.clone(), b.clone()]).unwrap();
    let target = db.search("Alpha merchant", None, 10).unwrap()[0]
        .merchant
        .id
        .clone();
    db.link("test", "external-b", &target).unwrap();
    db.import(&[b]).unwrap();
    let linked = db.search("Beta merchant", None, 10).unwrap();
    assert_eq!(linked[0].merchant.id, target);
    assert_eq!(linked[0].provenance.len(), 2);
    let mut bad = record("bad", " ");
    bad.merchant.id = "".into();
    assert!(db.import(&[record("new", "Brand new"), bad]).is_err());
    assert!(
        !db.search("Brand new", None, 10)
            .unwrap()
            .iter()
            .any(|c| c.exact)
    );
}
#[test]
fn negative_aliases_exclude_imported_candidates() {
    let db = MerchantStore::temporary().unwrap();
    let mut a = record("amazon", "Amazon");
    a.merchant.aliases = vec!["Amazon web services".into()];
    a.raw = serde_json::json!({"negativeAliases":["amazon web services"]});
    db.import(&[a]).unwrap();
    assert!(
        db.search("Amazon web services", None, 10)
            .unwrap()
            .is_empty()
    );
}
#[test]
fn normalization_fuzzy_retrieval_and_market_preference() {
    let db = MerchantStore::temporary().unwrap();
    db.put(&merchant("a", "Julius Café", "CA")).unwrap();
    db.put(&merchant("b", "Julius Café", "US")).unwrap();
    let exact = db.search("JULIUS CAFE", Some("CA"), 10).unwrap();
    assert_eq!(exact[0].merchant.id, "a");
    assert!(exact[0].exact);
    assert_eq!(exact.len(), 2);
    assert_eq!(
        db.search("Julus cafe", Some("CA"), 10).unwrap()[0]
            .merchant
            .id,
        "a"
    );
    assert_eq!(
        db.search("Juli", Some("CA"), 10).unwrap()[0].merchant.id,
        "a"
    );
    assert!(db.search("LS", Some("CA"), 10).unwrap().is_empty());
    assert_eq!(db.search("Julius cafe", None, 1).unwrap().len(), 2);
}
#[test]
fn updates_replace_aliases_and_handle_query_syntax_as_data() {
    let db = MerchantStore::temporary().unwrap();
    let mut m = merchant("a", "Julius Café", "CA");
    m.aliases = vec!["OLD DESCRIPTION".into()];
    db.put(&m).unwrap();
    m.aliases = vec!["NEW DESCRIPTION".into()];
    db.put(&m).unwrap();
    assert!(
        !db.search("OLD DESCRIPTION", None, 10)
            .unwrap()
            .iter()
            .any(|c| c.exact)
    );
    assert!(db.search("NEW DESCRIPTION", None, 10).unwrap()[0].exact);
    db.search("\" OR * (NEAR) --", None, 10).unwrap();
}

#[test]
fn alias_spans_retrieve_specific_domains_beyond_common_word_pool() -> Result<()> {
    let store = MerchantStore::temporary()?;
    // More distractors than the single-word and pair-query limits, inserted first.
    for index in 0..110 {
        store.put(&merchant(&format!("amazon-{index:03}"), "Amazon", "US"))?;
    }
    store.put(&merchant("amazon-ca", "Amazon.ca", "CA"))?;
    store.put(&merchant("toronto", "Toronto", "CA"))?;
    let request = serde_json::from_value(serde_json::json!({
        "description":"Amazon.ca*353873PQ3 TORONTO ON"
    }))?;
    let (hits, trace) = store.search_request_traced(&request, 10)?;
    assert_eq!(hits[0].merchant.id, "amazon-ca");
    assert!(!hits[0].exact && !hits[0].trusted);
    assert!(
        hits[0]
            .retrieval_evidence
            .iter()
            .any(|e| e.path == "alias_span" && e.query == "amazon ca")
    );
    assert!(
        trace
            .considered
            .iter()
            .any(|c| c.merchant_id == "toronto" && c.location_only)
    );
    assert!(hits.iter().filter(|c| c.merchant.name == "Amazon").count() >= 2);
    // No new learned aliases or resolutions are created by search.
    assert!(store.resolutions(None, 100)?.is_empty());
    assert!(
        !store
            .get("amazon-ca")?
            .unwrap()
            .aliases
            .contains(&request.description)
    );
    Ok(())
}

#[test]
fn name_budget_preserves_competing_identities_and_short_specific_names() -> Result<()> {
    let store = MerchantStore::temporary()?;
    for index in 0..12 {
        store.put(&merchant(&format!("a-{index:02}"), "Northstar", "CA"))?;
    }
    store.put(&merchant("specific", "Northstar.ca", "CA"))?;
    let request = serde_json::from_value(
        serde_json::json!({"description":"Northstar.ca*12AB34CD TORONTO ON"}),
    )?;
    let hits = store.search_request(&request, 2)?;
    assert_eq!(hits[0].merchant.id, "specific");
    assert_eq!(
        hits.iter()
            .filter(|c| c.merchant.name == "Northstar")
            .count(),
        12
    );
    assert!(hits.iter().all(|c| !c.exact && !c.trusted));
    Ok(())
}

#[test]
fn complete_alias_spans_outrank_generic_fragments_without_merging() -> Result<()> {
    let store = MerchantStore::temporary()?;
    for (id, name) in [
        ("full", "Northstar Coffee Roasters"),
        ("fragment", "Northstar"),
        ("suffix", "Coffee Roasters"),
        ("other", "Cardinal Coffee"),
    ] {
        store.put(&merchant(id, name, "CA"))?;
    }
    let request = serde_json::from_value(
        serde_json::json!({"description":"Northstar Coffee Roasters*123ABC TORONTO ON"}),
    )?;
    let (hits, trace) = store.search_request_traced(&request, 10)?;
    assert_eq!(hits[0].merchant.id, "full");
    assert!(!hits[0].exact && !hits[0].trusted);
    for id in ["fragment", "suffix"] {
        let evidence = trace
            .considered
            .iter()
            .find(|c| c.merchant_id == id)
            .unwrap();
        assert_eq!(
            evidence.shadowed_by.as_deref(),
            Some("northstar coffee roasters")
        );
        assert!(evidence.score <= 0.34);
    }
    assert!(store.get("fragment")?.is_some());
    // Full raw exact names retain their ordinary priority and trust behavior.
    let request = serde_json::from_value(serde_json::json!({"description":"Northstar"}))?;
    let hits = store.search_request(&request, 1)?;
    assert_eq!(hits[0].merchant.id, "fragment");
    assert!(hits[0].exact && hits[0].trusted);
    Ok(())
}
