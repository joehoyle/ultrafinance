use super::*;
#[test]
fn global_coverage_region_bridges_and_language_aliases() {
    assert!(metadata()["records"].as_u64().unwrap() > 180_000);
    assert!(metadata()["countries"].as_u64().unwrap() > 200);
    assert!(!lookup("Bromont", Some("CAN"), Some("QC")).is_empty());
    assert!(!lookup("Bondi Beach", Some("AU"), Some("NSW")).is_empty());
    assert!(!lookup("Montréal", Some("CA"), Some("Québec")).is_empty());
    assert!(!lookup("Munich", Some("DE"), None).is_empty());
    assert!(!lookup("München", Some("DE"), None).is_empty());
    assert!(!lookup("Amsterdam", Some("NL"), Some("NH")).is_empty());
    assert!(!lookup("Ft Lauderdale", Some("US"), Some("FL")).is_empty());
    assert!(!lookup("London", Some("GB"), None).is_empty());
    assert!(!lookup("Tokyo", Some("JP"), None).is_empty());
    assert!(lookup("Toronto", Some("CA"), Some("08")).is_empty());
    assert!(lookup("Toronto", Some("CA"), Some("NY")).is_empty());
    assert!(lookup("Toronto", Some("ZZ"), None).is_empty());
}
#[test]
fn ambiguity_is_preserved_and_ids_are_stable() {
    let cities = lookup("Springfield", Some("USA"), None);
    assert!(cities.len() > 1);
    assert!(
        cities
            .windows(2)
            .all(|pair| pair[0].geoname_id < pair[1].geoname_id)
    );
    let specific = lookup("Springfield", Some("US"), Some("IL"));
    assert!(!specific.is_empty());
    assert!(specific.len() < cities.len());
    assert!(specific.iter().all(|city| city.region == "IL"));
}
