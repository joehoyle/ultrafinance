//! Versioned, bundled GeoNames name index. No network or catalog database required.
use crate::store::normalize;
use serde::Serialize;
use std::{collections::HashMap, sync::OnceLock};

#[derive(Debug, Serialize)]
pub struct Locality {
    pub geoname_id: u32,
    pub city: &'static str,
    pub region: &'static str,
    pub country: &'static str,
    pub country_alpha3: &'static str,
    #[serde(skip_serializing)]
    region_aliases: &'static str,
}

struct Index {
    cities: Vec<Locality>,
    names: HashMap<&'static str, Vec<usize>>,
    countries: HashMap<&'static str, &'static str>,
    max_city_words: usize,
    max_region_words: usize,
}

pub fn metadata() -> &'static serde_json::Value {
    static METADATA: OnceLock<serde_json::Value> = OnceLock::new();
    METADATA.get_or_init(|| {
        serde_json::from_str(include_str!("../data/geonames/manifest.json"))
            .expect("generated gazetteer manifest")
    })
}

fn index() -> &'static Index {
    static INDEX: OnceLock<Index> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut index = Index {
            cities: Vec::new(),
            names: HashMap::new(),
            countries: HashMap::new(),
            max_city_words: 1,
            max_region_words: 1,
        };
        for line in include_str!("../data/geonames/cities.tsv").lines() {
            let fields: Vec<_> = line.split('\t').collect();
            assert_eq!(fields.len(), 7, "generated gazetteer row");
            let row = index.cities.len();
            for alias in fields[6].split('|') {
                index.max_city_words = index.max_city_words.max(alias.split_whitespace().count());
                index.names.entry(alias).or_default().push(row);
            }
            for alias in fields[5].split('|') {
                index.max_region_words =
                    index.max_region_words.max(alias.split_whitespace().count());
            }
            index.countries.insert(fields[2], fields[2]);
            index.countries.insert(fields[3], fields[2]);
            index.cities.push(Locality {
                geoname_id: fields[0].parse().expect("generated GeoNames ID"),
                city: fields[1],
                country: fields[2],
                country_alpha3: fields[3],
                region: fields[4],
                region_aliases: fields[5],
            });
        }
        index
    })
}

pub(crate) fn country(code: &str) -> Option<&'static str> {
    index()
        .countries
        .get(code.to_ascii_uppercase().as_str())
        .copied()
}

pub(crate) fn limits() -> (usize, usize) {
    (index().max_city_words, index().max_region_words)
}

pub(crate) fn region_matches(city: &Locality, region: &str) -> bool {
    let region = normalize(region);
    city.region_aliases.split('|').any(|alias| alias == region)
}

/// Exact normalized aliases, with country and region as constraints. Never rank
/// ambiguous cities by population or turn arbitrary FIPS codes into regions.
pub fn lookup(
    city: &str,
    country_code: Option<&str>,
    region: Option<&str>,
) -> Vec<&'static Locality> {
    let key = normalize(city);
    let country_code = match country_code {
        Some(code) => match country(code) {
            Some(country) => Some(country),
            None => return vec![],
        },
        None => None,
    };
    let mut aliases = vec![key.clone()];
    if let Some(rest) = key.strip_prefix("ft ") {
        aliases.push(format!("fort {rest}"));
    }
    let mut found: Vec<_> = aliases
        .iter()
        .filter_map(|alias| index().names.get(alias.as_str()))
        .flatten()
        .map(|&row| &index().cities[row])
        .filter(|city| {
            country_code.is_none_or(|country| city.country == country)
                && region.is_none_or(|region| region_matches(city, region))
        })
        .collect();
    found.sort_by_key(|city| city.geoname_id);
    found.dedup_by_key(|city| city.geoname_id);
    found
}

/// Browse deterministic ID order without consulting merchant outlet records.
pub fn list(
    country_code: Option<&str>,
    region: Option<&str>,
    limit: usize,
    offset: usize,
) -> (usize, Vec<&'static Locality>) {
    let country_code = match country_code {
        Some(code) => match country(code) {
            Some(country) => Some(country),
            None => return (0, vec![]),
        },
        None => None,
    };
    let matching = index().cities.iter().filter(|city| {
        country_code.is_none_or(|country| city.country == country)
            && region.is_none_or(|region| region_matches(city, region))
    });
    let total = matching.clone().count();
    (total, matching.skip(offset).take(limit).collect())
}

#[cfg(test)]
mod tests {
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
}
