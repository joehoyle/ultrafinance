use std::collections::HashMap;

use futures::{stream, StreamExt};
use serde::Deserialize;

use crate::{Location, NewMerchant, Transaction};

pub struct Client {
    reqwest: reqwest::Client,
    api_key: String,
}

#[derive(Deserialize, Debug)]
struct Response {
    merchant: String,
    merchant_id: String,
    website: Option<String>,
    icon: Option<String>,
    address: Option<Address>,
}

#[derive(Deserialize, Debug, Clone)]
struct Address {
    line1: Option<String>,
    city: Option<String>,
    state: Option<String>,
    #[serde(rename = "postalCode")]
    post_code: Option<String>,
    country: Option<String>,
}

impl From<Address> for Location {
    fn from(address: Address) -> Self {
        Location {
            address: address.line1,
            city: address.city,
            state: address.state,
            postcode: address.post_code,
            country: address.country,
            latitude: None,
            longitude: None,
            google_maps_url: None,
            apple_maps_url: None,
            store_number: None,
        }
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}, {}, {}, {}, {}",
            self.line1.clone().unwrap_or("".to_string()),
            self.city.clone().unwrap_or("".to_string()),
            self.state.clone().unwrap_or("".to_string()),
            self.post_code.clone().unwrap_or("".to_string()),
            self.country.clone().unwrap_or("".to_string())
        )
    }
}

impl From<Response> for NewMerchant {
    fn from(response: Response) -> Self {
        NewMerchant {
            name: response.merchant,
            website: response.website,
            logo_url: response.icon,
            external_id: Some(response.merchant_id),
            location: response.address.clone().map(|l| l.to_string()),
            location_structured: response.address.map(|l| l.into()),
            labels: None,
        }
    }
}

impl Client {
    pub fn new(api_key: String) -> Self {
        Client {
            reqwest: reqwest::Client::new(),
            api_key,
        }
    }

    pub async fn get_merchants(
        &self,
        transactions: &Vec<Transaction>,
    ) -> Result<HashMap<u32, NewMerchant>, anyhow::Error> {
        let max_concurrent_requests = 10; // Limit the number of concurrent requests

        let requests = stream::iter(transactions.iter().map(|transaction| {
            let url = format!("https://api.synthfinance.com/enrich?description");
            let reqwest_client = self.reqwest.clone();
            let api_key = self.api_key.clone();
            let transaction_id = transaction.id;
            let query_params = vec![
                (
                    "description",
                    format!(
                        "{} {} {}",
                        transaction.creditor_name.clone().unwrap_or("".to_string()),
                        transaction.debtor_name.clone().unwrap_or("".to_string()),
                        transaction
                            .remittance_information
                            .clone()
                            .unwrap_or("".to_string())
                    ),
                ),
                ("amount", transaction.transaction_amount.clone()),
                (
                    "country",
                    transaction
                        .transaction_amount_currency
                        .used_by()
                        .first()
                        .unwrap_or(&"")
                        .to_string(),
                ),
            ];

            async move {
                let response = reqwest_client
                    .get(&url)
                    .query(&query_params)
                    .header("Authorization", format!("Bearer {}", api_key))
                    .send()
                    .await;

                match response {
                    Ok(response) => {
                        let response = response.json::<Response>().await;
                        match response {
                            Ok(response) => {
                                let merchant = NewMerchant::from(response);
                                Some((transaction_id, merchant))
                            }
                            Err(e) => {
                                log::error!("Error parsing merchant response: {}", e);
                                None
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Error fetching merchant: {}", e);
                        None
                    }
                }
            }
        }))
        .buffer_unordered(max_concurrent_requests);

        let results = requests.collect::<Vec<_>>().await;
        let mut enriched = HashMap::new();
        for result in results {
            if let Some((transaction_id, merchant)) = result {
                enriched.insert(transaction_id, merchant);
            }
        }

        Ok(enriched.into_iter().collect())
    }
}
