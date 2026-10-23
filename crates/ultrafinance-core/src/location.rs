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

/// Source provenance retained when several source records describe one outlet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationProvenance {
    pub source: String,
    pub external_id: String,
    pub attribution: String,
    pub license: String,
    pub url: String,
    pub transaction_pattern: Option<String>,
}
impl From<&LocationRecord> for LocationProvenance {
    fn from(r: &LocationRecord) -> Self {
        Self {
            source: r.source.clone(),
            external_id: r.external_id.clone(),
            attribution: r.attribution.clone(),
            license: r.license.clone(),
            url: r.url.clone(),
            transaction_pattern: r.transaction_pattern.clone(),
        }
    }
}

/// Reviewed outlet knowledge, imported explicitly. Source keys preserve local IDs
/// on refresh. Neither company addresses nor all upstream children are outlets.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocationRecord {
    /// Populated on consolidated reads; raw source records remain separately stored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<LocationProvenance>,
    pub source: String,
    pub external_id: String,
    pub merchant: MerchantReference,
    pub location: LocationData,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub transaction_pattern: Option<String>,
    /// Accept and discard the removed field when reading older exports.
    #[doc(hidden)]
    #[serde(default, skip_serializing, rename = "place_ids")]
    pub legacy_place_ids: serde::de::IgnoredAny,
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

fn descriptor_geography(description: &str) -> LocationHint {
    if crate::descriptor_location::billing_description(description) {
        return LocationHint::default();
    }
    let request: EnrichRequest =
        serde_json::from_value(serde_json::json!({"description": description}))
            .expect("descriptor request");
    let extracted = crate::descriptor_location::extract(description, &request);
    let mut result = LocationHint::default();
    if let [location] = extracted.as_slice()
        && location.geoname_ids.len() <= 1
    {
        result = location.hint.clone();
        if let Some(city) = location.canonical_city {
            result.city = Some(city.into());
        }
    }
    result.store_number = crate::descriptor_location::store_number(description);
    result
}

pub(crate) fn compatible(a: &LocationHint, b: &LocationHint) -> bool {
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

#[cfg(test)]
pub(crate) fn enrich(
    request: &EnrichRequest,
    outlets: &[LocationRecord],
) -> (LocationResult, Vec<String>) {
    enrich_for_merchant(request, outlets, None)
}

pub(crate) fn enrich_for_merchant(
    request: &EnrichRequest,
    outlets: &[LocationRecord],
    merchant: Option<&crate::Merchant>,
) -> (LocationResult, Vec<String>) {
    let extracted = descriptor_geography(&request.description);
    let mut geography = request.location.clone().unwrap_or_default();
    // Conflicting descriptor geography is discarded as a group, avoiding mixed
    // cities/regions. Transaction country can exclude an outlet, but alone does not invent a place.
    if compatible(&geography, &extracted) {
        macro_rules! fill { ($($field:ident),*) => { $(if geography.$field.is_none() { geography.$field = extracted.$field; })* }; }
        fill!(address, city, region, postal_code, country, store_number);
    }
    // A descriptor locality need not appear in the general geographic dataset
    // when the recognized merchant's own outlets independently support it.
    if geography.city.is_none()
        && !crate::descriptor_location::billing_description(&request.description)
        && let Some(merchant) = merchant
    {
        let interpretation = crate::interpretation::interpret_merchant(request);
        let mut cities = std::collections::BTreeMap::new();
        for city in interpretation.hypotheses.iter().filter_map(|hypothesis| {
            crate::interpretation::supporting_name(merchant, &hypothesis.merchant_text)?;
            hypothesis.possible_location.as_deref()
        }) {
            for outlet in outlets {
                let place = outlet.location.geography();
                if let Some(catalog_city) = &place.city
                    && normalize(city) == normalize(catalog_city)
                    && compatible(&geography, &place)
                    && !request.country.as_ref().is_some_and(|country| {
                        place
                            .country
                            .as_ref()
                            .is_none_or(|stored| country != stored)
                    })
                {
                    cities.insert(normalize(catalog_city), catalog_city.clone());
                }
            }
        }
        if cities.len() == 1 {
            geography.city = cities.into_values().next();
        }
    }
    let query = normalize(&request.description);
    // A trailing country code can filter catalog outlets even without a city.
    // Keep it out of extracted geography: a country hint alone is not a place.
    let descriptor_country =
        (!crate::descriptor_location::billing_description(&request.description))
            .then(|| request.description.split_whitespace().last())
            .flatten()
            .and_then(|word| crate::gazetteer::country(&normalize(word)));
    let eligible: Vec<_> = outlets
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
            if descriptor_country
                .zip(place.country.as_deref())
                .is_some_and(|(a, b)| a != b)
            {
                return false;
            }
            true
        })
        .collect();
    let matched: Vec<_> = eligible
        .iter()
        .copied()
        .filter(|record| {
            let place = record.location.geography();
            let alias = record.aliases.iter().any(|a| normalize(a) == query);
            let pattern = record
                .transaction_pattern
                .iter()
                .chain(
                    record
                        .provenance
                        .iter()
                        .filter_map(|p| p.transaction_pattern.as_ref()),
                )
                .any(|p| regex_rules::match_length(p, &request.description).is_some());
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
            // Geography narrows the matched merchant's outlets before deciding
            // whether additional descriptor evidence is needed.
            eligible.len() == 1 || alias || pattern || store || address
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
        let merchant = merchant_id
            .as_deref()
            .map(|id| store.get(id))
            .transpose()?
            .flatten();
        let outlets = merchant_id
            .map(|id| store.locations(&id))
            .transpose()?
            .unwrap_or_default();
        let (predicted, _) = enrich_for_merchant(&case.request, &outlets, merchant.as_ref());
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
#[path = "../tests/unit/location/tests.rs"]
mod tests;
