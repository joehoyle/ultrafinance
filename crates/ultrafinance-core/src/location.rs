//! Conservative transaction geography and independently sourced merchant outlets.
use crate::{EnrichRequest, regex_rules, store::normalize};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct LocationHint {
    pub address: Option<String>,
    pub city: Option<String>,
    /// State/province or other administrative region; not restricted to US codes.
    pub region: Option<String>,
    pub postal_code: Option<String>,
    /// ISO 3166-1 alpha-2 code, in uppercase.
    pub country: Option<String>,
    /// Merchant-scoped string; preserves leading zeros and letters.
    pub store_number: Option<String>,
}
impl LocationHint {
    pub fn validate(&self) -> Result<()> {
        if self
            .country
            .as_ref()
            .is_some_and(|s| s.len() != 2 || !s.bytes().all(|b| b.is_ascii_uppercase()))
        {
            bail!("location.country must be a two-letter uppercase code");
        }
        for value in [
            &self.address,
            &self.city,
            &self.region,
            &self.postal_code,
            &self.store_number,
        ]
        .into_iter()
        .flatten()
        {
            if value.trim().is_empty() || value.len() > 512 {
                bail!("location fields must contain 1 to 512 bytes of nonblank text");
            }
        }
        Ok(())
    }
    fn has_data(&self) -> bool {
        self.address.is_some()
            || self.city.is_some()
            || self.region.is_some()
            || self.postal_code.is_some()
            || self.country.is_some()
            || self.store_number.is_some()
    }
    fn precision(&self) -> Option<LocationPrecision> {
        if self.address.is_some() {
            Some(LocationPrecision::Address)
        } else if self.city.is_some() {
            Some(LocationPrecision::City)
        } else if self.region.is_some() {
            Some(LocationPrecision::Region)
        } else if self.country.is_some() {
            Some(LocationPrecision::Country)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum LocationPrecision {
    Country,
    Region,
    City,
    Address,
    Outlet,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct LocationData {
    /// Service catalog ID; null for extracted geography.
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Null when only a store number or postal code is known.
    pub precision: Option<LocationPrecision>,
    pub address: Option<String>,
    pub city: Option<String>,
    pub region: Option<String>,
    pub postal_code: Option<String>,
    pub country: Option<String>,
    pub store_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latitude: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub longitude: Option<f64>,
}

impl LocationData {
    fn geography(&self) -> LocationHint {
        LocationHint {
            address: self.address.clone(),
            city: self.city.clone(),
            region: self.region.clone(),
            postal_code: self.postal_code.clone(),
            country: self.country.clone(),
            store_number: self.store_number.clone(),
        }
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LocationResult {
    Matched {
        data: LocationData,
    },
    /// Geography supported by input or descriptor, without an identified outlet.
    Extracted {
        data: LocationData,
    },
    Unresolved {
        #[cfg_attr(feature = "openapi", schema(schema_with = crate::null_schema))]
        data: (),
    },
}
impl Default for LocationResult {
    fn default() -> Self {
        Self::Unresolved { data: () }
    }
}

/// Stable merchant reference; source references follow explicit merchant links.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum MerchantReference {
    Local { merchant_id: String },
    Source { source: String, external_id: String },
}

/// Reviewed outlet knowledge, imported explicitly. Source keys preserve local IDs
/// on refresh. Neither company addresses nor all upstream children are outlets.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocationRecord {
    pub source: String,
    pub external_id: String,
    pub merchant: MerchantReference,
    pub location: LocationData,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub transaction_pattern: Option<String>,
    /// External place identifiers retained as evidence, not service catalog IDs.
    #[serde(default)]
    pub place_ids: std::collections::BTreeMap<String, String>,
    /// Protect a reviewed correction from later source refreshes.
    #[serde(default)]
    pub manual_override: bool,
    pub attribution: String,
    pub license: String,
    pub url: String,
}
impl LocationRecord {
    pub fn validate(&self) -> Result<()> {
        self.location.geography().validate()?;
        if self.source.trim().is_empty()
            || self.external_id.trim().is_empty()
            || self.attribution.trim().is_empty()
            || self.license.trim().is_empty()
            || self.url.trim().is_empty()
        {
            bail!("outlets require source/external ID and provenance");
        }
        if self.location.precision != Some(LocationPrecision::Outlet)
            || self.location.address.is_none()
            || self.location.country.is_none()
        {
            bail!("catalog locations must be outlets with an address and country");
        }
        if self.location.latitude.is_some() != self.location.longitude.is_some()
            || self
                .location
                .latitude
                .is_some_and(|v| !v.is_finite() || !(-90.0..=90.0).contains(&v))
            || self
                .location
                .longitude
                .is_some_and(|v| !v.is_finite() || !(-180.0..=180.0).contains(&v))
        {
            bail!("outlet coordinates must be a valid latitude/longitude pair");
        }
        if self
            .aliases
            .iter()
            .any(|a| normalize(a).chars().count() < 3 || a.len() > 4096)
        {
            bail!(
                "outlet aliases must contain at least three normalized characters and at most 4096 bytes"
            );
        }
        if self.place_ids.iter().any(|(key, value)| {
            key.trim().is_empty() || value.trim().is_empty() || key.len() > 64 || value.len() > 512
        }) {
            bail!("place IDs must have nonblank provider names and identifiers");
        }
        if let Some(pattern) = &self.transaction_pattern
            && !regex_rules::valid_pattern(pattern)
        {
            bail!("invalid or unsupported outlet transaction pattern");
        }
        if self.aliases.is_empty() && self.transaction_pattern.is_none() {
            bail!("outlets require an alias or transaction pattern");
        }
        Ok(())
    }
}

// Explicit locality/region/country triples avoid guessing where the merchant
// name stops and the city begins. Extend using reviewed examples, not suffix words.
const LOCALITIES: &[(&str, &str, &str)] = &[
    ("HIALEAH", "FL", "US"),
    ("MIAMI", "FL", "US"),
    ("FT LAUDERDALE", "FL", "US"),
    ("FORT LAUDERDALE", "FL", "US"),
    ("ORLANDO", "FL", "US"),
    ("LAKE BUENA VISTA", "FL", "US"),
    ("NEW YORK", "NY", "US"),
    ("SAN FRANCISCO", "CA", "US"),
    ("LOS ANGELES", "CA", "US"),
    ("SAN DIEGO", "CA", "US"),
    ("CHICAGO", "IL", "US"),
    ("HOUSTON", "TX", "US"),
    ("TORONTO", "ON", "CA"),
    ("MONTREAL", "QC", "CA"),
    ("BROMONT", "QC", "CA"),
    ("VANCOUVER", "BC", "CA"),
    ("OTTAWA", "ON", "CA"),
    ("CALGARY", "AB", "CA"),
    ("CANBERRA", "ACT", "AU"),
    ("SYDNEY", "NSW", "AU"),
    ("BONDI BEACH", "NSW", "AU"),
    ("CHIPPENDALE", "NSW", "AU"),
];

fn descriptor_geography(description: &str) -> LocationHint {
    let text = normalize(description).to_ascii_uppercase();
    // These descriptors frequently carry billing/processor cities, not purchase
    // geography. Structured caller evidence can still be supplied separately.
    let billing = [
        "GOOGLE",
        "PAYPAL",
        "SUBSCRIPTION",
        "SUBSCR",
        "ONLINE",
        "OPENAI",
        "ANTHROPIC",
        "CLAUDE",
        "HULU",
        "NETFLIX",
        "DISNEY PLUS",
        "QANTAS",
    ];
    let padded = format!(" {text} ");
    if billing.iter().any(|s| padded.contains(&format!(" {s} "))) || text.contains("APPLE COM BILL")
    {
        return LocationHint::default();
    }
    let mut result = LocationHint::default();
    for &(city, region, country) in LOCALITIES {
        let suffix = format!("{city} {region}");
        if [
            &suffix,
            &format!("{suffix} {country}"),
            &format!(
                "{suffix} {}",
                if country == "US" {
                    "USA"
                } else if country == "AU" {
                    "AUS"
                } else {
                    "CAN"
                }
            ),
        ]
        .into_iter()
        .any(|suffix| text == *suffix || text.ends_with(&format!(" {suffix}")))
        {
            result.city = Some(
                city.split_whitespace()
                    .map(|word| {
                        let mut chars = word.chars();
                        format!(
                            "{}{}",
                            chars.next().unwrap(),
                            chars.as_str().to_ascii_lowercase()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            result.region = Some(region.into());
            result.country = Some(country.into());
            break;
        }
    }
    // Do not extract an arbitrary numeric token: dates, cards, and phones abound.
    static STORE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let store =
        STORE.get_or_init(|| regex::Regex::new(r"(?i)(?:#|\bSTORE\s+)([A-Z0-9]{1,16})\b").unwrap());
    let identifiers: std::collections::BTreeSet<_> = store
        .captures_iter(description)
        .filter_map(|captures| {
            let found = captures.get(0)?;
            let prefix = &description[..found.start()];
            // A hash embedded in a card/reference token is not a store marker.
            if found.as_str().starts_with('#')
                && prefix.chars().last().is_some_and(|c| !c.is_whitespace())
            {
                return None;
            }
            let preceding = prefix
                .split_whitespace()
                .last()
                .unwrap_or("")
                .to_ascii_uppercase();
            if [
                "CARD", "ACCOUNT", "AUTH", "ORDER", "REF", "RECEIPT", "PHONE",
            ]
            .contains(&preceding.as_str())
            {
                return None;
            }
            Some(captures[1].to_ascii_uppercase())
        })
        .collect();
    if identifiers.len() == 1 {
        result.store_number = identifiers.into_iter().next();
    }
    result
}

fn compatible(a: &LocationHint, b: &LocationHint) -> bool {
    // Caller evidence is authoritative for conflict detection. No partial
    // matches (e.g. "123 Main" versus "123 Main Street") are assumed.
    [
        (&a.address, &b.address),
        (&a.city, &b.city),
        (&a.region, &b.region),
        (&a.country, &b.country),
        (&a.postal_code, &b.postal_code),
        (&a.store_number, &b.store_number),
    ]
    .into_iter()
    .all(|(a, b)| match (a, b) {
        (Some(a), Some(b)) => normalize(a) == normalize(b),
        _ => true,
    })
}

pub(crate) fn enrich(
    request: &EnrichRequest,
    outlets: &[LocationRecord],
) -> (LocationResult, Vec<String>) {
    let extracted = descriptor_geography(&request.description);
    let mut geography = request.location.clone().unwrap_or_default();
    // Conflicting descriptor geography is discarded as a group, avoiding mixed
    // cities/regions. Transaction country can exclude an outlet, but alone does not invent a place.
    if compatible(&geography, &extracted) {
        macro_rules! fill { ($($field:ident),*) => { $(if geography.$field.is_none() { geography.$field = extracted.$field; })* }; }
        fill!(address, city, region, postal_code, country, store_number);
    }
    let query = normalize(&request.description);
    let matched: Vec<_> = outlets
        .iter()
        .filter(|record| {
            let place = record.location.geography();
            if request
                .country
                .as_ref()
                .zip(place.country.as_ref())
                .is_some_and(|(a, b)| a != b)
            {
                return false;
            }
            if !compatible(&geography, &place) {
                return false;
            }
            let alias = record.aliases.iter().any(|a| normalize(a) == query);
            let pattern = record
                .transaction_pattern
                .as_ref()
                .is_some_and(|p| regex_rules::match_length(p, &request.description).is_some());
            // A merchant-scoped store number or structured street address can
            // identify an outlet without a matching descriptor alias.
            let store = geography.store_number.is_some()
                && place.store_number.is_some()
                && geography.store_number.as_ref().map(|s| normalize(s))
                    == place.store_number.as_ref().map(|s| normalize(s));
            let address = geography.address.is_some()
                && place.address.is_some()
                && geography.address.as_ref().map(|s| normalize(s))
                    == place.address.as_ref().map(|s| normalize(s));
            alias || pattern || store || address
        })
        .collect();
    if let [record] = matched.as_slice() {
        return (
            LocationResult::Matched {
                data: record.location.clone(),
            },
            vec![format!(
                "{} ({}, {})",
                record.attribution, record.license, record.url
            )],
        );
    }
    if geography.has_data() {
        let precision = geography.precision();
        (
            LocationResult::Extracted {
                data: LocationData {
                    id: None,
                    name: None,
                    precision,
                    address: geography.address,
                    city: geography.city,
                    region: geography.region,
                    postal_code: geography.postal_code,
                    country: geography.country,
                    store_number: geography.store_number,
                    latitude: None,
                    longitude: None,
                },
            },
            vec![],
        )
    } else {
        (LocationResult::default(), vec![])
    }
}

/// Offline location evaluation isolates geography/outlet matching from merchant
/// resolution. A supplied merchant reference is ground truth for outlet scoping.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocationSuite {
    pub name: String,
    pub cases: Vec<LocationCase>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocationCase {
    pub id: String,
    pub request: EnrichRequest,
    pub merchant: Option<MerchantReference>,
    pub expected: ExpectedLocation,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExpectedLocation {
    Matched {
        source: String,
        external_id: String,
        fields: LocationHint,
    },
    Extracted {
        fields: LocationHint,
    },
    Unresolved,
}
#[derive(Serialize)]
pub struct LocationReport {
    pub suite: String,
    pub cases: usize,
    pub geographic_fields: usize,
    pub correct_geographic_fields: usize,
    pub geographic_field_accuracy: Option<f64>,
    pub labeled_outlets: usize,
    pub correct_outlets: usize,
    pub outlet_accuracy: Option<f64>,
    pub labeled_unresolved: usize,
    pub correct_unresolved: usize,
    pub unresolved_accuracy: Option<f64>,
    pub results: Vec<serde_json::Value>,
}

pub fn evaluate(
    store: &crate::store::MerchantStore,
    suite: LocationSuite,
) -> Result<LocationReport> {
    if suite.name.trim().is_empty() || suite.cases.is_empty() {
        bail!("location suites require a name and cases");
    }
    let mut ids = std::collections::HashSet::new();
    let mut report = LocationReport {
        suite: suite.name,
        cases: suite.cases.len(),
        geographic_fields: 0,
        correct_geographic_fields: 0,
        geographic_field_accuracy: None,
        labeled_outlets: 0,
        correct_outlets: 0,
        outlet_accuracy: None,
        labeled_unresolved: 0,
        correct_unresolved: 0,
        unresolved_accuracy: None,
        results: vec![],
    };
    for case in suite.cases {
        if case.id.trim().is_empty() || !ids.insert(case.id.clone()) {
            bail!("location case IDs must be nonblank and unique");
        }
        case.request.validate()?;
        let merchant_id = match case.merchant {
            Some(MerchantReference::Local { merchant_id }) => Some(merchant_id),
            Some(MerchantReference::Source {
                source,
                external_id,
            }) => store.resolve_source(&source, &external_id)?,
            None => None,
        };
        let outlets = merchant_id
            .map(|id| store.locations(&id))
            .transpose()?
            .unwrap_or_default();
        let (predicted, _) = enrich(&case.request, &outlets);
        let actual = match &predicted {
            LocationResult::Matched { data } | LocationResult::Extracted { data } => {
                data.geography()
            }
            LocationResult::Unresolved { .. } => LocationHint::default(),
        };
        let outcome_correct = match &case.expected {
            ExpectedLocation::Matched {
                source,
                external_id,
                ..
            } => {
                report.labeled_outlets += 1;
                let correct = match &predicted {
                    LocationResult::Matched { data } => outlets.iter().any(|r| {
                        r.source == *source
                            && r.external_id == *external_id
                            && r.location.id == data.id
                    }),
                    _ => false,
                };
                report.correct_outlets += usize::from(correct);
                correct
            }
            ExpectedLocation::Extracted { .. } => {
                matches!(predicted, LocationResult::Extracted { .. })
            }
            ExpectedLocation::Unresolved => {
                report.labeled_unresolved += 1;
                let correct = matches!(predicted, LocationResult::Unresolved { .. });
                report.correct_unresolved += usize::from(correct);
                correct
            }
        };
        let fields_correct = match &case.expected {
            ExpectedLocation::Matched { fields, .. } | ExpectedLocation::Extracted { fields } => {
                fields.validate()?;
                if !fields.has_data() {
                    bail!("location field labels must not be empty");
                }
                let mut correct = true;
                for (expected, actual) in [
                    (&fields.address, &actual.address),
                    (&fields.city, &actual.city),
                    (&fields.region, &actual.region),
                    (&fields.country, &actual.country),
                    (&fields.postal_code, &actual.postal_code),
                    (&fields.store_number, &actual.store_number),
                ] {
                    if let Some(expected) = expected {
                        report.geographic_fields += 1;
                        let matches = actual
                            .as_ref()
                            .is_some_and(|actual| normalize(actual) == normalize(expected));
                        report.correct_geographic_fields += usize::from(matches);
                        correct &= matches;
                    }
                }
                correct
            }
            ExpectedLocation::Unresolved => true,
        };
        report.results.push(serde_json::json!({"id":case.id,"expected":case.expected,"predicted":predicted,"correct":outcome_correct && fields_correct}));
    }
    fn ratio(n: usize, d: usize) -> Option<f64> {
        (d > 0).then(|| n as f64 / d as f64)
    }
    report.geographic_field_accuracy =
        ratio(report.correct_geographic_fields, report.geographic_fields);
    report.outlet_accuracy = ratio(report.correct_outlets, report.labeled_outlets);
    report.unresolved_accuracy = ratio(report.correct_unresolved, report.labeled_unresolved);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Enricher, MerchantResult, store::MerchantStore};
    use serde_json::json;

    fn request(description: &str) -> EnrichRequest {
        serde_json::from_value(json!({"description":description})).unwrap()
    }
    fn records() -> Vec<LocationRecord> {
        serde_json::from_str(include_str!(
            "../../../data/locations/open-enrichment-au.json"
        ))
        .unwrap()
    }
    fn seed(store: &MerchantStore) {
        let records = crate::import::catalog(
            include_str!("../../../data/locations/merchants.example.json"),
            "open-enrichment",
        )
        .unwrap();
        store.import(&records).unwrap();
    }

    #[test]
    fn geography_extracts_localities_and_preserves_store_identifiers() {
        let (result, _) = enrich(&request("POLLO TROPICAL #10241 HIALEAH FL"), &[]);
        let LocationResult::Extracted { data } = result else {
            panic!("expected extraction")
        };
        assert_eq!(data.city.as_deref(), Some("Hialeah"));
        assert_eq!(data.region.as_deref(), Some("FL"));
        assert_eq!(data.country.as_deref(), Some("US"));
        assert_eq!(data.store_number.as_deref(), Some("10241"));
        assert_eq!(data.precision, Some(LocationPrecision::City));
        assert!(data.id.is_none() && data.latitude.is_none() && data.longitude.is_none());
        let (result, _) = enrich(&request("WALMART #0006"), &[]);
        let LocationResult::Extracted { data } = result else {
            panic!("expected store identifier")
        };
        assert_eq!(data.store_number.as_deref(), Some("0006"));
        assert_eq!(data.precision, None);
        let (result, _) = enrich(&request("CAFE TORONTO ON CA"), &[]);
        let LocationResult::Extracted { data } = result else {
            panic!("expected extraction")
        };
        assert_eq!(data.country.as_deref(), Some("CA"));
    }

    #[test]
    fn billing_cities_phones_truncation_and_country_hints_do_not_invent_places() {
        for description in [
            "GOOGLE *YOUTUBE MOUNTAIN VIEW CA",
            "OPENAI SAN FRANCISCO CA",
            "PAYPAL *CARVANA 402-935-7733 AZ",
            "HULU LOS ANGELES CA",
            "CAFE CA",
            "LIDL GB NOTTINGHA",
            "ONLINE PURCHASE TORONTO ON",
            "SQ *CAFE 123456",
            "CARD#1234",
            "CARD #1234",
            "AUTH #1234",
            "CAFE #123 #456",
        ] {
            let mut req = request(description);
            req.country = Some("CA".into());
            assert_eq!(
                enrich(&req, &[]).0,
                LocationResult::default(),
                "{description}"
            );
        }
    }

    #[test]
    fn structured_input_is_independent_and_conflicts_do_not_mix_geography() {
        let mut req = request("CAFE HIALEAH FL");
        req.country = Some("CA".into());
        req.location =
            Some(serde_json::from_value(json!({"city":"Toronto", "country":"CA"})).unwrap());
        let LocationResult::Extracted { data } = enrich(&req, &[]).0 else {
            panic!("expected input evidence")
        };
        assert_eq!(data.city.as_deref(), Some("Toronto"));
        assert_eq!(data.country.as_deref(), Some("CA"));
        assert!(data.region.is_none());
        for hint in [
            json!({"country":"ca"}),
            json!({"city":" "}),
            json!({"store_number":"x".repeat(513)}),
        ] {
            req.location = Some(serde_json::from_value(hint).unwrap());
            assert!(req.validate().is_err());
        }
        assert!(
            serde_json::from_value::<EnrichRequest>(
                json!({"description":"x","location":{"citty":"Toronto"}})
            )
            .is_err()
        );
    }

    #[test]
    fn outlets_need_specific_evidence_and_ambiguity_or_conflicts_abstain() {
        let mut outlets = records();
        for (index, record) in outlets.iter_mut().enumerate() {
            record.location.id = Some(format!("loc_{index}"));
        }
        let (result, credits) = enrich(&request("APPLE STORE R483 R483 CANBERRA"), &outlets);
        let LocationResult::Matched { data } = result else {
            panic!("expected outlet")
        };
        assert_eq!(data.id.as_deref(), Some("loc_0"));
        assert_eq!(data.country.as_deref(), Some("AU"));
        assert_eq!(data.precision, Some(LocationPrecision::Outlet));
        assert_eq!(credits.len(), 1);
        assert_eq!(
            enrich(&request("APPLE.COM/BILL SYDNEY"), &outlets).0,
            LocationResult::default()
        );
        assert_eq!(
            enrich(&request("APPLE STORE"), &outlets).0,
            LocationResult::default()
        );
        assert!(!matches!(
            enrich(&request("APPLE STORE R483 SYDNEY NSW AU"), &outlets).0,
            LocationResult::Matched { .. }
        ));
        let mut overseas = request("APPLE STORE R483 R483 CANBERRA");
        overseas.country = Some("CA".into());
        assert!(!matches!(
            enrich(&overseas, &outlets).0,
            LocationResult::Matched { .. }
        ));
        overseas.country = Some("AU".into());
        assert!(matches!(
            enrich(&overseas, &outlets).0,
            LocationResult::Matched { .. }
        ));
        outlets.push(outlets[0].clone());
        assert!(!matches!(
            enrich(&request("APPLE STORE R483"), &outlets).0,
            LocationResult::Matched { .. }
        ));
        let mut req = request("APPLE");
        req.location =
            Some(serde_json::from_value(json!({"store_number":"R238","country":"AU"})).unwrap());
        assert!(matches!(
            enrich(&req, &records()).0,
            LocationResult::Matched { .. }
        ));
    }

    #[test]
    fn imports_are_atomic_stable_and_follow_merchant_links() {
        let store = MerchantStore::memory().unwrap();
        seed(&store);
        let merchant_id = store
            .resolve_source("open-enrichment", "04eb38e9-d678-4d44-90a9-1786f12effd6")
            .unwrap()
            .unwrap();
        let records = records();
        store.import_locations(&records).unwrap();
        let before = store.locations(&merchant_id).unwrap();
        assert_eq!(before.len(), 2);
        assert!(before[0].location.id.as_ref().unwrap().starts_with("loc_"));
        store.import_locations(&records).unwrap();
        assert_eq!(
            before[0].location.id,
            store.locations(&merchant_id).unwrap()[0].location.id
        );
        let mut bad = records[0].clone();
        bad.external_id = "bad".into();
        bad.merchant = MerchantReference::Local {
            merchant_id: "missing".into(),
        };
        let mut good = records[0].clone();
        good.external_id = "would-be-new".into();
        assert!(store.import_locations(&[good, bad]).is_err());
        assert_eq!(store.locations(&merchant_id).unwrap().len(), 2);
        let mut replacement = store.list(None, 10, 0).unwrap().merchants.remove(0);
        replacement.id = "mer_linked".into();
        store.put(&replacement).unwrap();
        store
            .link(
                "open-enrichment",
                "04eb38e9-d678-4d44-90a9-1786f12effd6",
                "mer_linked",
            )
            .unwrap();
        assert_eq!(store.locations("mer_linked").unwrap().len(), 2);
        assert!(store.locations(&merchant_id).unwrap().is_empty());
    }

    #[tokio::test]
    async fn exact_batch_and_unresolved_merchants_get_independent_locations_and_logs() {
        let store = MerchantStore::memory().unwrap();
        seed(&store);
        let mut merchant = store.list(None, 10, 0).unwrap().merchants.remove(0);
        merchant
            .aliases
            .push("APPLE STORE R483 R483 CANBERRA".into());
        store.put(&merchant).unwrap();
        store.import_locations(&records()).unwrap();
        let enricher =
            Enricher::with_store(None, "jev-latest".into(), 0.95, store.clone()).unwrap();
        let results = enricher
            .enrich_batch(&[
                request("APPLE STORE R483 R483 CANBERRA"),
                request("ZXQ #0006 HIALEAH FL"),
                request("ZXQ"),
            ])
            .await;
        assert!(matches!(
            results[0].as_ref().unwrap().merchant,
            MerchantResult::Matched { .. }
        ));
        assert!(matches!(
            results[0].as_ref().unwrap().location,
            LocationResult::Matched { .. }
        ));
        assert!(matches!(
            results[1].as_ref().unwrap().merchant,
            MerchantResult::Unresolved { .. }
        ));
        assert!(matches!(
            results[1].as_ref().unwrap().location,
            LocationResult::Extracted { .. }
        ));
        assert_eq!(
            results[2].as_ref().unwrap().location,
            LocationResult::default()
        );
        let logs = store.enrichment_logs(None, None, 10, 0).unwrap();
        assert!(
            logs.iter()
                .any(|r| r["data"]["response"]["location"]["status"] == "matched")
        );
        let serialized = serde_json::to_value(results[0].as_ref().unwrap()).unwrap();
        let roundtrip: crate::EnrichResponse = serde_json::from_value(serialized).unwrap();
        assert_eq!(roundtrip.location, results[0].as_ref().unwrap().location);
    }

    #[test]
    fn evaluation_measures_fields_outlets_and_abstention_separately() {
        let store = MerchantStore::memory().unwrap();
        seed(&store);
        store.import_locations(&records()).unwrap();
        let suite: LocationSuite =
            serde_json::from_str(include_str!("../../../evals/location-smoke.json")).unwrap();
        let report = evaluate(&store, suite).unwrap();
        assert_eq!(report.geographic_field_accuracy, Some(1.0));
        assert_eq!(report.outlet_accuracy, Some(1.0));
        assert_eq!(report.unresolved_accuracy, Some(1.0));
        let mut suite: LocationSuite =
            serde_json::from_str(include_str!("../../../evals/location-smoke.json")).unwrap();
        suite.cases[0].request.description = "ZXQ".into();
        suite.cases[9].request.description = "APPLE STORE R9999".into();
        let report = evaluate(&store, suite).unwrap();
        assert!(report.geographic_field_accuracy.unwrap() < 1.0);
        assert_eq!(report.outlet_accuracy, Some(0.5));
    }

    #[test]
    fn source_refresh_preserves_manual_corrections_and_fingerprint_tracks_outlets() {
        let store = MerchantStore::memory().unwrap();
        seed(&store);
        let before = store.fingerprint().unwrap();
        store.import_locations(&records()).unwrap();
        assert_ne!(before, store.fingerprint().unwrap());
        let mut manual = records().remove(0);
        manual.manual_override = true;
        manual.location.name = Some("Reviewed name".into());
        store.import_locations(&[manual]).unwrap();
        store.import_locations(&records()).unwrap();
        let merchant_id = store
            .resolve_source("open-enrichment", "04eb38e9-d678-4d44-90a9-1786f12effd6")
            .unwrap()
            .unwrap();
        assert!(
            store
                .locations(&merchant_id)
                .unwrap()
                .iter()
                .any(|r| r.location.name.as_deref() == Some("Reviewed name"))
        );
    }

    #[test]
    fn reviewed_record_validation_rejects_bad_geometry_and_patterns() {
        let mut record = records().remove(0);
        record.validate().unwrap();
        record.location.longitude = None;
        assert!(record.validate().is_err());
        record.location.longitude = Some(200.0);
        assert!(record.validate().is_err());
        record.location.longitude = Some(149.0);
        record.transaction_pattern = Some(".*".into());
        assert!(record.validate().is_err());
        record.transaction_pattern = Some("(?=APPLE)".into());
        assert!(record.validate().is_err());
        record.transaction_pattern = None;
        record.aliases = vec!["APPLE".into()];
        record.location.precision = Some(LocationPrecision::City);
        assert!(record.validate().is_err());
    }
}
