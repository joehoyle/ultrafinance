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
    let (records, _) = prepare(input, "ca")?;
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
fn preparation_preserves_per_place_identities_and_is_order_independent() {
    let rows = [
        (
            "a",
            "Northstar Books Downtown",
            "CA",
            "https://northstar.test",
            "",
        ),
        (
            "b",
            "Northstar Books Uptown",
            "CA",
            "https://northstar.test",
            "",
        ),
        ("c", "Central Cafe", "CA", "https://example.test", ""),
        ("d", "Central Cafe", "CA", "https://example.test", ""),
    ];
    let (records, _) = prepare(&csv(&rows), "ca").unwrap();
    assert_eq!(records.len(), 4);
    for record in &records {
        assert!(record.external_id.starts_with("place:"));
        assert_eq!(record.raw["places"].as_array().unwrap().len(), 1);
        assert!(record.raw.get("review_evidence").is_none());
        assert!(record.merchant.aliases.is_empty());
    }
    let mut reversed = rows;
    reversed.reverse();
    assert_eq!(
        serde_json::to_value(records).unwrap(),
        serde_json::to_value(prepare(&csv(&reversed), "ca").unwrap().0).unwrap()
    );
}
#[test]
fn conflicting_place_ids_and_invalid_regions_fail() {
    let input = csv(&[("a", "A", "CA", "", ""), ("a", "B", "CA", "", "")]);
    assert!(prepare(&input, "global").is_err());
    let input = csv(&[("a", "A", "CA", "", "")]);
    assert!(prepare(&input, "../").is_err());
}
#[test]
fn invalid_countries_skip_only_the_affected_places() {
    let input = csv(&[
        ("a", "Before", "CA", "", ""),
        ("b", "Missing", "", "", ""),
        ("c", "Long", "CAN", "", ""),
        ("d", "Nonletter", "C1", "", ""),
        ("e", "Malformed", "??", "", ""),
        ("f", "After", " ca ", "", ""),
    ]);
    for region in ["global", "ca"] {
        let (records, skipped) = prepare(&input, region).unwrap();
        assert_eq!(skipped, 4);
        assert_eq!(
            records
                .iter()
                .map(|r| r.external_id.as_str())
                .collect::<Vec<_>>(),
            vec!["place:a", "place:f"]
        );
        for record in records {
            assert_eq!(record.merchant.markets, vec!["CA"]);
            crate::store::validate(&record.merchant).unwrap();
        }
    }
}

#[test]
fn invalid_upstream_websites_preserve_merchants_and_original_evidence() {
    let input = csv(&[
        ("a", "A", "CA", "ftp://example.com", ""),
        ("b", "B", "CA", "https://user:password@example.com", ""),
        ("c", "C", "CA", "example.com", ""),
        ("d", "D", "CA", "https://example.com", ""),
    ]);
    let (records, skipped) = prepare(&input, "ca").unwrap();
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
}
#[test]
fn closure_country_and_noncommercial_filters_do_not_create_merchants() {
    let input = csv(&[
        ("a", "A", "CA", "", "2020-01-01"),
        ("b", "B", "US", "", ""),
        ("c", "C", "CA", "", ""),
    ]);
    let (records, skipped) = prepare(&input, "ca").unwrap();
    assert_eq!(skipped, 2);
    assert_eq!(records.len(), 1);
    let input = input.replace("restaurant", "530e33ccbcbc57f1066bbff7");
    assert!(prepare(&input, "global").unwrap().0.is_empty());
}
#[test]
fn unmatchable_names_are_skipped() {
    let input = csv(&[
        ("a", "---", "CA", "", ""),
        ("b", "☕", "CA", "", ""),
        ("c", "Café", "CA", "", ""),
        ("d", "東京", "CA", "", ""),
    ]);
    let (records, skipped) = prepare(&input, "ca").unwrap();
    assert_eq!(skipped, 2);
    assert_eq!(records.len(), 2);
    for record in &records {
        crate::store::validate(&record.merchant).unwrap();
    }
}

#[test]
fn unusable_generated_outlet_aliases_preserve_merchants_without_aborting() -> Result<()> {
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record([
        "fsq_place_id",
        "name",
        "country",
        "address",
        "fsq_category_ids",
        "date_closed",
    ])?;
    // The first case is the upstream row that stopped the global import.
    for (id, name, address) in [
        ("a", "+1", "\u{e332}"),
        ("b", "AB", "---"),
        ("c", "東京", "☕"),
        ("d", "A", "1"),
        ("e", "Cafe", "1 Main St"),
    ] {
        writer.write_record([id, name, "SG", address, "[\"restaurant\"]", ""])?;
    }
    let (records, skipped) = prepare(&String::from_utf8(writer.into_inner()?)?, "global")?;
    assert_eq!(records.len(), 5);
    assert_eq!(skipped, 0);
    for record in &records[..3] {
        crate::store::validate(&record.merchant)?;
        assert!(locations(record)?.is_empty());
        assert!(
            !record.raw["places"][0]["address"]
                .as_str()
                .unwrap()
                .is_empty()
        );
    }
    for record in &records[3..] {
        let outlets = locations(record)?;
        assert_eq!(outlets.len(), 1);
        outlets[0].validate()?;
    }
    assert_eq!(locations(&records[3])?[0].aliases, vec!["A 1"]);
    Ok(())
}
