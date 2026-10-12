use super::*;
#[test]
fn studio_keeps_provenance_and_country_hints_without_claiming_identity() {
    let input = r#"{"schemaVersion":"1.1.0","generatedAt":"2026-07-06","merchants":[{"id":"amazon","canonicalName":"Amazon","aliases":["amzn mktp"],"website":"amazon.com","countryHints":["US","CA"],"negativeAliases":["aws"]}]}"#;
    let records = merchant_studio(input).unwrap();
    assert_eq!(records[0].source, "merchant-studio");
    assert_eq!(records[0].external_id, "amazon");
    assert!(records[0].merchant.markets.is_empty());
    assert_eq!(
        records[0].merchant.website.as_deref(),
        Some("https://amazon.com")
    );
    assert_eq!(records[0].raw["countryHints"][1], "CA");
    assert_eq!(records[0].license, "CC-BY-4.0");
    assert!(merchant_studio(r#"{"schemaVersion":"2.0","merchants":[]}"#).is_err());
}
