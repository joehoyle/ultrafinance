use super::*;
#[test]
fn bank_merchant_hint_adds_a_query_without_replacing_the_descriptor() {
    let request = serde_json::from_value(serde_json::json!({
        "description":"POS 2232 NORTHSTAR GRANBY QC",
        "extra":{"bank":{"merchant_name":"Northstar Books"}}
    }))
    .unwrap();
    let parsed = interpret(&request);
    assert_eq!(parsed.original, "POS 2232 NORTHSTAR GRANBY QC");
    assert!(
        parsed
            .hypotheses
            .iter()
            .any(|h| h.merchant_text == "Northstar Books")
    );
    assert!(
        parsed
            .hypotheses
            .iter()
            .any(|h| h.merchant_text == parsed.original)
    );
}
#[test]
fn terminal_store_numbers_are_hypotheses_after_locality_extraction() {
    let request = serde_json::from_value(
        serde_json::json!({"description":"NORTHSTAR BOOKS - 2232 GRANBY QC"}),
    )
    .unwrap();
    let parsed = interpret(&request);
    assert!(parsed.hypotheses.iter().any(|h| {
        normalize(&h.merchant_text) == "northstar books"
            && h.location_hint
                .as_ref()
                .and_then(|h| h.city.as_deref())
                .map(normalize)
                .as_deref()
                == Some("granby")
    }));
    assert!(
        parsed
            .hypotheses
            .iter()
            .any(|h| h.merchant_text.contains("2232"))
    );
    let request =
        serde_json::from_value(serde_json::json!({"description":"STUDIO 54 GRANBY QC"})).unwrap();
    assert!(
        interpret(&request)
            .hypotheses
            .iter()
            .any(|h| normalize(&h.merchant_text) == "studio 54")
    );
}
#[test]
fn global_gazetteer_hypotheses_preserve_ambiguous_ids_and_region_context() {
    for description in ["CAFE / Springfield / USA", "CAFE SPRINGFIELD USA"] {
        let request =
            serde_json::from_value(serde_json::json!({"description":description})).unwrap();
        let parsed = interpret(&request);
        let hypothesis = parsed
            .hypotheses
            .iter()
            .find(|h| h.geoname_ids.len() > 1)
            .unwrap();
        assert_eq!(hypothesis.merchant_text, "CAFE");
        assert_eq!(
            hypothesis
                .location_hint
                .as_ref()
                .unwrap()
                .country
                .as_deref(),
            Some("US")
        );
        assert!(hypothesis.location_hint.as_ref().unwrap().region.is_none());
        assert_eq!(parsed.original, description);
    }
    let request =
        serde_json::from_value(serde_json::json!({"description":"CAFE SPRINGFIELD IL USA"}))
            .unwrap();
    let parsed = interpret(&request);
    let hint = parsed
        .hypotheses
        .iter()
        .find_map(|h| h.location_hint.as_ref())
        .unwrap();
    assert_eq!(hint.region.as_deref(), Some("IL"));
    for description in [
        "CAFE / München / DEU",
        "CAFE TOKYO JPN",
        "CAFE LONDON GBR",
        "CAFE SYDNEY NEW SOUTH WALES AUS",
    ] {
        let request =
            serde_json::from_value(serde_json::json!({"description":description})).unwrap();
        assert!(
            interpret(&request)
                .hypotheses
                .iter()
                .any(|h| h.merchant_text == "CAFE" && !h.geoname_ids.is_empty()),
            "{description}"
        );
    }
}
#[test]
fn structured_locations_validate_whole_suffix_and_preserve_names() {
    for (description, city, region, country, store) in [
        (
            "SQ *CAFE STORE 00482 TORONTO ON CAN",
            "TORONTO",
            Some("ON"),
            "CA",
            Some("00482"),
        ),
        (
            "CAFE TORONTO ON #00482",
            "TORONTO",
            Some("ON"),
            "CA",
            Some("00482"),
        ),
        ("CAFE TORONTO ON 123456", "TORONTO", Some("ON"), "CA", None),
        (
            "CAFE SAN FRANCISCO CA",
            "SAN FRANCISCO",
            Some("CA"),
            "US",
            None,
        ),
        ("CAFE NEW YORK NY USA", "NEW YORK", Some("NY"), "US", None),
        ("CAFE MONTRÉAL QC CA", "MONTRÉAL", Some("QC"), "CA", None),
        (
            "CAFE BONDI BEACH NSW AUS",
            "BONDI BEACH",
            Some("NSW"),
            "AU",
            None,
        ),
        ("CAFE / Amsterdam / NLD", "Amsterdam", None, "NL", None),
        ("CAFE/Amsterdam/NL", "Amsterdam", None, "NL", None),
        ("CAFE AMSTERDAM NLD", "AMSTERDAM", None, "NL", None),
    ] {
        let request =
            serde_json::from_value(serde_json::json!({"description":description})).unwrap();
        let parsed = interpret(&request);
        assert_eq!(parsed.original, description);
        let hypothesis = parsed
            .hypotheses
            .iter()
            .find(|h| h.location_hint.is_some())
            .unwrap_or_else(|| panic!("{description}"));
        assert_eq!(hypothesis.merchant_text, "CAFE", "{description}");
        let hint = hypothesis.location_hint.as_ref().unwrap();
        assert_eq!(hint.city.as_deref(), Some(city), "{description}");
        assert_eq!(hint.region.as_deref(), region, "{description}");
        assert_eq!(hint.country.as_deref(), Some(country), "{description}");
        assert_eq!(hint.store_number.as_deref(), store, "{description}");
        assert!(hint.address.is_none() && hint.postal_code.is_none());
        assert!(
            parsed
                .hypotheses
                .iter()
                .any(|h| h.merchant_text == description && h.location_hint.is_none())
        );
    }
    let request =
        serde_json::from_value(serde_json::json!({"description":"CAFE STORE 0006"})).unwrap();
    let parsed = interpret(&request);
    let hint = parsed
        .hypotheses
        .iter()
        .find_map(|h| h.location_hint.as_ref())
        .unwrap();
    assert_eq!(hint.store_number.as_deref(), Some("0006"));
    assert!(hint.city.is_none() && hint.region.is_none() && hint.country.is_none());
}

#[test]
fn locations_reject_conflicts_billing_and_ambiguous_tokens() {
    for description in [
        "CAFE TORONTO NY",
        "CAFE TORONTO ON USA",
        "CAFE UNKNOWNCITY ON CAN",
        "CAFE / Unknowncity / CAN",
        "CAFE BROMONT",
        "CAFE CA",
        "CAFE 123456",
        "CAFE CARD #1234",
        "CAFE AUTH #1234",
        "CAFE #123 #456",
        "PAYPAL *CAFE TORONTO ON",
        "PP*CAFE TORONTO ON",
        "OPENAI SAN FRANCISCO CA",
        "ONLINE CAFE TORONTO ON",
        "GOOGLE *CAFE TORONTO ON",
    ] {
        let request =
            serde_json::from_value(serde_json::json!({"description":description})).unwrap();
        assert!(
            interpret(&request)
                .hypotheses
                .iter()
                .all(|h| h.location_hint.is_none()),
            "{description}"
        );
    }
    for context in [
        serde_json::json!({"country":"US"}),
        serde_json::json!({"location":{"country":"US"}}),
        serde_json::json!({"location":{"city":"Montreal", "country":"CA"}}),
        serde_json::json!({"location":{"region":"QC", "country":"CA"}}),
    ] {
        let mut value = context;
        value["description"] = serde_json::json!("CAFE TORONTO ON CAN");
        let request = serde_json::from_value(value).unwrap();
        assert!(
            interpret(&request)
                .hypotheses
                .iter()
                .all(|h| h.location_hint.is_none())
        );
    }
}

#[test]
fn catalog_location_support_requires_compatible_and_unique_outlets() {
    let request = serde_json::from_value(serde_json::json!({
        "description":"JULIUS CAFE STORE 00482 TORONTO ON CAN"
    }))
    .unwrap();
    let parsed = interpret(&request);
    let merchant = serde_json::from_value(serde_json::json!({
        "id":"cafe", "name":"Julius Cafe", "markets":["CA"]
    }))
    .unwrap();
    let record: crate::location::LocationRecord = serde_json::from_value(serde_json::json!({
        "source":"reviewed-outlets", "external_id":"toronto-482", "aliases":["Julius Cafe Toronto"],
        "merchant":{"merchant_id":"cafe"},
        "location":{"id":null,"precision":"outlet", "city":"Toronto", "region":"ON",
            "country":"CA", "store_number":"00482"},
        "attribution":"Test fixture", "license":"test", "url":"https://example.com/outlet"
    }))
    .unwrap();
    let evidence = catalog_support(&parsed, &merchant, std::slice::from_ref(&record), None);
    assert_eq!(
        evidence
            .iter()
            .find_map(|e| e.outlet.as_ref())
            .unwrap()
            .external_id,
        "toronto-482"
    );
    for (region, country, store) in [
        ("QC", "CA", "00482"),
        ("ON", "US", "00482"),
        ("ON", "CA", "00483"),
    ] {
        let mut conflicting = record.clone();
        conflicting.location.region = Some(region.into());
        conflicting.location.country = Some(country.into());
        conflicting.location.store_number = Some(store.into());
        assert!(
            catalog_support(&parsed, &merchant, &[conflicting], None)
                .iter()
                .all(|e| e.outlet.is_none())
        );
    }
    assert!(
        catalog_support(
            &parsed,
            &merchant,
            std::slice::from_ref(&record),
            Some("US")
        )
        .iter()
        .all(|e| e.outlet.is_none())
    );
    let mut other = record.clone();
    other.external_id = "other-outlet".into();
    assert!(
        catalog_support(&parsed, &merchant, &[record, other], None)
            .iter()
            .all(|e| e.outlet.is_none())
    );
}

#[test]
fn ambiguous_localities_and_numbers_are_preserved_as_hypotheses() {
    let request =
        serde_json::from_value(serde_json::json!({"description":"SQ *JULIUS CAFE BROMONT 00482"}))
            .unwrap();
    let parsed = interpret(&request);
    assert_eq!(parsed.processor_hint.as_deref(), Some("Square"));
    assert_eq!(parsed.unverified_tokens, ["00482"]);
    assert_eq!(parsed.hypotheses[0].merchant_text, "JULIUS CAFE BROMONT");
    assert_eq!(parsed.hypotheses[1].merchant_text, "JULIUS CAFE");
    assert_eq!(
        parsed.hypotheses[1].possible_location.as_deref(),
        Some("BROMONT")
    );
    for description in ["7 ELEVEN", "STUDIO 54", "東京 カフェ", "LS"] {
        let request =
            serde_json::from_value(serde_json::json!({"description":description})).unwrap();
        assert_eq!(interpret(&request).hypotheses[0].merchant_text, description);
    }
}

#[test]
fn bank_formats_preserve_original_and_extract_merchant_hypotheses() {
    for (description, merchant, processor) in [
        ("CHECKCARD 1001 CARVANA REF: AB123", "CARVANA", None),
        ("31/12/2026 CARVANA", "CARVANA", None),
        (
            "CARVANA DES :PAYMENT INDN :RECIPIENT CO ID :123",
            "CARVANA",
            None,
        ),
        (
            "10/01 POS PURCHASE SQ *JULIUS CAFE 2026-10-01 REF: AB1234",
            "JULIUS CAFE",
            Some("Square"),
        ),
        ("PAYPAL *CARVANA 402-935-7733 AZ", "CARVANA", Some("PayPal")),
        ("PP* CARVANA (402) 935-7733", "CARVANA", Some("PayPal")),
        ("TST * JULIUS CAFE", "JULIUS CAFE", Some("Toast")),
        ("SP * STUDIO 54 ORDER #A1234", "STUDIO 54", Some("Shopify")),
        (
            "PADDLE.NET* SOFTWARE REF:ABC123",
            "SOFTWARE",
            Some("Paddle"),
        ),
        ("gosq.com JULIUS CAFE", "JULIUS CAFE", Some("Square")),
        ("SQ * PAYPAL * JULIUS CAFE", "JULIUS CAFE", Some("Square")),
        (
            "Orig CO Name:JULIUS CAFE Orig ID:123456 Desc Date:261001 CO Entry Descr:PAYMENT Sec:CCD Trace#:123456789 Eed:261002 Ind Name:RECIPIENT",
            "JULIUS CAFE",
            None,
        ),
        (
            "JULIUS CAFE DES:PAYMENT ID:123456 INDN:RECIPIENT CO ID:987654 PPD",
            "JULIUS CAFE",
            None,
        ),
        (
            "Adyen Tech Support / Amsterdam / NLD",
            "Adyen Tech Support",
            None,
        ),
        ("東京 カフェ 2026/10/01 REF: X123", "東京 カフェ", None),
    ] {
        let request =
            serde_json::from_value(serde_json::json!({"description":description})).unwrap();
        let parsed = interpret(&request);
        assert_eq!(parsed.original, description);
        assert_eq!(parsed.processor_hint.as_deref(), processor, "{description}");
        assert!(
            parsed
                .hypotheses
                .iter()
                .any(|h| h.merchant_text == merchant),
            "{description}: {:?}",
            parsed.hypotheses
        );
        assert!(
            parsed
                .hypotheses
                .iter()
                .any(|h| h.merchant_text == description),
            "{description}"
        );
        assert!(
            !parsed
                .hypotheses
                .iter()
                .any(|h| h.merchant_text == "RECIPIENT")
        );
    }
}

#[test]
fn ambiguous_names_dates_references_and_processor_boundaries_survive() {
    for description in [
        "7 ELEVEN",
        "STUDIO 54",
        "FOREVER 21",
        "SQUID CAFE",
        "SPAR",
        "LS",
        "PAYPAL",
        "SQ*",
        "ACME DES: STUDIO",
        "ACME REF: BOOKS",
        "ACME A10/01B",
        "CAFE 99/99",
        "CAFE 13/99/2026",
        "CAFE 2026-99-10",
    ] {
        let request =
            serde_json::from_value(serde_json::json!({"description":description})).unwrap();
        let parsed = interpret(&request);
        assert_eq!(parsed.hypotheses[0].merchant_text, description);
        assert!(parsed.processor_hint.is_none(), "{description}");
    }
    let description = "POS PURCHASE 10/01 REF: AB123";
    let request = serde_json::from_value(serde_json::json!({"description":description})).unwrap();
    let parsed = interpret(&request);
    assert_eq!(parsed.hypotheses.len(), 1);
    assert_eq!(parsed.hypotheses[0].merchant_text, description);
    assert!(parsed.unverified_tokens.iter().any(|s| s == "REF: AB123"));
}

#[test]
fn delimited_reference_adds_name_hypothesis_and_keeps_original() {
    let request = serde_json::from_value(serde_json::json!({
        "description":"Amazon.ca*353873PQ3 TORONTO ON"
    }))
    .unwrap();
    let parsed = interpret(&request);
    assert!(
        parsed
            .hypotheses
            .iter()
            .any(|h| h.merchant_text == "Amazon.ca"
                && h.location_hint.as_ref().and_then(|l| l.country.as_deref()) == Some("CA"))
    );
    assert!(
        parsed
            .hypotheses
            .iter()
            .any(|h| h.merchant_text == request.description)
    );
    assert!(parsed.unverified_tokens.contains(&"*353873PQ3".into()));
    for description in ["STUDIO 54", "7 ELEVEN", "NOVEL*PREMIUM", "NOVEL*123"] {
        let request =
            serde_json::from_value(serde_json::json!({"description":description})).unwrap();
        assert!(
            interpret(&request).unverified_tokens.is_empty(),
            "{description}"
        );
    }
}
