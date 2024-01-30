use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::models;
use crate::NewMerchant;
use crate::Transaction;

pub struct ApiClient {
    async_client: reqwest::Client,
}

impl ApiClient {
    pub fn new(api_key: String) -> Self {
        let async_client = reqwest::Client::builder()
            .default_headers({
                let mut headers = reqwest::header::HeaderMap::new();
                headers.insert(
                    "X-API-KEY",
                    reqwest::header::HeaderValue::from_str(&api_key).unwrap(),
                );
                headers
            })
            .build()
            .unwrap();
        ApiClient {
            async_client,
        }
    }

    /// Enrich and add transactions to the ledger of account holders synchronously.
    ///
    /// Add transactions to the ledgers of account holders and get back enriched version of the transactions in the response. Accepts batch sizes up to 4k transactions. A single transaction should take ~100ms. A batch of 4000 transactions should take ~40s.
    pub async fn async_enrich_transactions(
        &self,
        transactions: Vec<TransactionInput>,
    ) -> Result<Vec<TransactionOutput>> {
        let url = "https://api.ntropy.com/v2/transactions/sync";

        let text = self
            .async_client
            .post(url)
            .json(&transactions)
            .send()
            .await?
            .text()
            .await?;
        let jd = &mut serde_json::Deserializer::from_str(&text);
        let result: Result<Vec<TransactionOutput>> =
            serde_path_to_error::deserialize(jd).map_err(|e| e.into());
        result
    }
}

#[derive(Serialize, Debug)]
pub struct TransactionInput {
    /// Description of the transaction.
    pub description: String,
    /// Direction of the flow of money from the perspective of the account holder. Possible values are incoming and outgoing.
    pub entry_type: String,
    /// Amount of the transaction.
    pub amount: f32,
    /// ISO currency code for the transaction.
    pub iso_currency_code: String,
    /// Date of the transaction.
    pub date: chrono::NaiveDate,
    /// Unique identifier for the transaction.
    pub transaction_id: String,
    /// Country of the transaction (optional).
    pub country: Option<String>,
    /// Account holder ID (optional).
    pub account_holder_id: Option<String>,
    /// Account holder type (optional).
    pub account_holder_type: Option<String>,
}

/// Represents the output of a transaction.
#[derive(Deserialize, Debug)]
pub struct TransactionOutput {
    /// Labels from our live hierarchy, depending on the type of account holder (consumer, business).
    pub labels: Vec<String>,
    /// Higher level category that groups together related labels.
    pub label_group: Option<String>,
    /// Indicates whether a transaction is a one-time transfer, e.g. purchasing a mattress (one-off),
    /// regularly repeats with personalized pricing, e.g. utilities, mortgage (recurring),
    /// regularly repeats with fixed pricing (subscription).
    pub recurrence: Option<String>,
    /// If a transaction is recurrent, this is a `RecurrenceGroup` object with fields described below,
    /// `None` if the transaction is not recurrent.
    // recurrence_group: Option<RecurrenceGroup>,
    /// Location of the transaction (if a location is present) as a formatted string.
    pub location: Option<String>,
    /// Location of the transaction (if a location is present) as a structured object.
    pub location_structured: Option<LocationStructured>,
    /// Logo of the merchant (if a merchant is present) in URL format.
    pub logo: Option<String>,
    /// Normalized merchant name (if a merchant is present).
    pub merchant: Option<String>,
    /// Unique merchant identifier (if a merchant is present).
    pub merchant_id: Option<String>,
    /// Name of the person in the transaction text (if a person is present).
    pub person: Option<String>,
    /// Unique transaction identifier.
    pub transaction_id: String,
    /// Website of the merchant (if a merchant is present).
    pub website: Option<String>,
    /// Predicted MCC codes, usually containing a single value.
    /// Can be multiple values if the merchant can operate with multiple MCCs (if a merchant is present).
    pub mcc: Option<Vec<i32>>,
}

/// Represents the structured location of a transaction.
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct LocationStructured {
    /// The street address (including house number, apartment, suite, unit, or building number, if applicable).
    pub address: Option<String>,
    /// City, district, suburb, town, or village.
    pub city: Option<String>,
    /// State, county, province, or region.
    pub state: Option<String>,
    /// ZIP or postal code.
    pub postcode: Option<String>,
    /// Two-letter country code (ISO 3166-1 alpha-2).
    pub country: Option<String>,
    /// Latitude of the location.
    pub latitude: Option<f32>,
    /// Longitude of the location.
    pub longitude: Option<f32>,
    /// Link to the location on Google Maps.
    pub google_maps_url: Option<String>,
    /// Link to the location on Apple Maps.
    pub apple_maps_url: Option<String>,
    /// Store number of the location if found in the transaction description.
    pub store_number: Option<f32>,
}

impl From<Transaction> for TransactionInput {
    fn from(transaction: Transaction) -> Self {
        let amount = transaction.transaction_amount.parse::<f32>().unwrap_or(0.0);
        TransactionInput {
            description: format!(
                "{} {} {}",
                transaction.creditor_name.unwrap_or("".to_string()),
                transaction.debtor_name.unwrap_or("".to_string()),
                transaction.remittance_information.unwrap_or("".to_string())
            ),
            entry_type: if amount < 0 as f32 {
                "outgoing".to_string()
            } else {
                "incoming".to_string()
            },
            amount: amount.abs(),
            iso_currency_code: transaction.transaction_amount_currency,
            date: transaction.booking_date,
            transaction_id: transaction.id.to_string(),
            country: None,
            account_holder_id: None,
            account_holder_type: None,
        }
    }
}

impl From<LocationStructured> for models::merchant::Location {
    fn from(value: LocationStructured) -> Self {
        models::merchant::Location {
            address: value.address,
            city: value.city,
            state: value.state,
            postcode: value.postcode,
            country: value.country,
            latitude: value.latitude,
            longitude: value.longitude,
            google_maps_url: value.google_maps_url,
            apple_maps_url: value.apple_maps_url,
            store_number: value.store_number,
        }
    }
}
impl TryFrom<&TransactionOutput> for NewMerchant {
    type Error = anyhow::Error;
    fn try_from(value: &TransactionOutput) -> Result<Self> {
        Ok(NewMerchant {
            name: value
                .merchant
                .clone()
                .ok_or(anyhow::anyhow!("No merchant"))?,
            logo_url: value.logo.clone(),
            location: value.location.clone(),
            location_structured: match &value.location_structured {
                Some(ls) => Some(ls.clone().into()),
                None => None,
            },
            labels: Some(value.labels.join(",")),
            external_id: value.merchant_id.clone(),
            website: value.website.clone(),
        })
    }
}
