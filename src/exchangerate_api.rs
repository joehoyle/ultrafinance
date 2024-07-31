use std::collections::HashMap;

use chrono::{Utc};
use serde::Deserialize;

use crate::{exchange_rate::ExchangeRate, ultrafinance::Currency};

pub struct Client {
	reqwest: reqwest::Client,
	api_key: String,
}

#[derive(Deserialize)]
struct Response {
	conversion_rates: HashMap<String, f64>,
	base_code: Currency,
}

impl From<Response> for ExchangeRate {
	fn from(response: Response) -> Self {
		let mut conversation_rates: HashMap<Currency, f64> = HashMap::new();
		for (key, value) in response.conversion_rates {
			if let Ok(currency) = Currency::try_from(key) {
				conversation_rates.insert(currency, value);
			}
		}
		ExchangeRate {
			base_code: response.base_code,
			conversion_rates: conversation_rates,
			last_update: Utc::now().naive_utc(),
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

	pub async fn get_exchange_rate(&self, currency: &Currency) -> Result<ExchangeRate, anyhow::Error> {
		let url = format!("https://v6.exchangerate-api.com/v6/{}/latest/{}", &self.api_key, &currency);
		let response = self.reqwest.get(&url).send().await?;
		let json = response.json::<Response>().await?;

		Ok(json.into())
	}
}
