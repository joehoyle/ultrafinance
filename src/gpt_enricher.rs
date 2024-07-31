use crate::{NewMerchant, Transaction};
use async_openai::{
    config::OpenAIConfig,
    types::{
        ChatCompletionFunctionsArgs,
        ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs, Role,
    },
};

use futures::{stream::FuturesUnordered, StreamExt};
use serde::Deserialize;
use std::collections::HashMap;

pub struct Client {
    openai: async_openai::Client<async_openai::config::OpenAIConfig>,
}

#[derive(Deserialize)]
struct EnrichTransactionArguments {
    transaction_id: u32,
    merchant: NewMerchant,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct BrandfetchResponse {
    brandId: String,
    icon: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct EnrichTransactionsArguments {
    enriched_transactions: Vec<EnrichTransactionArguments>,
}

impl Client {
    pub fn new(api_key: String) -> Self {
        let config = OpenAIConfig::new().with_api_key(api_key);
        let openai = async_openai::Client::with_config(config);
        Client { openai }
    }

    pub async fn get_merchants(
        &self,
        transactions: &Vec<Transaction>,
    ) -> Result<HashMap<u32, NewMerchant>, anyhow::Error> {
        let request = CreateChatCompletionRequestArgs::default()
        .model("gpt-4-turbo-preview")
        .messages([
            ChatCompletionRequestUserMessageArgs::default()
                .role(Role::System)
                .content("You are a financial transaction enriching service. You accept JSON encoded transactions and call the enrich_transactions function with the Merchant data for the transaction. Generated the most approprate merchant data based off the transaction data you receive.")
                .build()?
                .into(),
            ChatCompletionRequestUserMessageArgs::default()
                .role(Role::User)
                .content(serde_json::to_string(transactions)?)
                .build()?
                .into()
            ])
        .functions([ChatCompletionFunctionsArgs::default()
            .name("enrich_transactions")
            .description("Enrich transactions with merchant data for each provided transaction.")
            .parameters(serde_json::json!({
                "type": "object",
                "properties": {
                    "enriched_transactions": {
                        "description": "The transactions that have been enriched. Their ID and the merchant data.",
                        "type": "array",
                        "items": {
                            "type": "object",
                            "description": "The transaction id / new merchant data pair.",
                            "properties": {
                                "transaction_id": {
                                    "type": "number"
                                },
                                "merchant": {
                                    "type": "object",
                                    "properties": {
                                        "name": {
                                            "type": "string"
                                        },
                                        "location": {
                                            "type": "object",
                                            "type": "string"
                                        },
                                        "location_structured": {
                                            "type": "object",
                                            "properties": {
                                                "address": {
                                                    "type": "string"
                                                },
                                                "city": {
                                                    "type": "string"
                                                },
                                                "state": {
                                                    "type": "string"
                                                },
                                                "postcode": {
                                                    "type": "string"
                                                },
                                                "country": {
                                                    "type": "string"
                                                },
                                                "latitude": {
                                                    "type": "number"
                                                },
                                                "longitude": {
                                                    "type": "number"
                                                }
                                            }
                                        },
                                        "labels": {
                                            "type": "string",
                                            "desscription": "space separated list of labels"
                                        },
                                        "website": {
                                            "type": "string"
                                        },
                                    },
                                    "required": ["name", "location", "location_structured", "labels", "website"]
                                }
                            },
                            "required": ["transaction_id", "merchant"],
                        }
                    },
                },
                "required": ["transactions"],
            }))
            .build()?])
        .function_call(r#"enrich_transactions"#)
        .build()?;

        dbg!(&request);
        let response_message = self
            .openai
            .chat()
            .create(request)
            .await?
            .choices
            .get(0)
            .ok_or(anyhow::anyhow!("No choices in response"))?
            .message
            .clone();
        dbg!(&response_message);
        #[allow(deprecated)]
        if let Some(function_call) = response_message.function_call {
            // let function_name = function_call.name;
            // let function_args: serde_json::Value = function_call.arguments.parse().unwrap();
            let mut transactions_map = HashMap::new();
            let transactions =
                serde_json::from_str::<EnrichTransactionsArguments>(&function_call.arguments)?;
            let mut futures = FuturesUnordered::new();
            for mut transaction in transactions.enriched_transactions {
                futures.push(async move {
                    transaction.merchant = self
                        .add_brandfetch(transaction.merchant.clone())
                        .await
                        .unwrap_or(transaction.merchant);
                    (transaction.transaction_id, transaction.merchant)
                });
            }

            while let Some((transaction_id, merchant)) = futures.next().await {
                // Perform the operation that does not need to be awaited
                transactions_map.insert(transaction_id, merchant);
                // You can use `id` here as needed
            }
            Ok(transactions_map)
        } else {
            Err(anyhow::anyhow!("No tool calls in response"))
        }
    }

    async fn add_brandfetch(
        &self,
        mut merchant: NewMerchant,
    ) -> Result<NewMerchant, anyhow::Error> {
        // Look up the merchant from brandfetch.com, as that data is better for logos etc
        let request = reqwest::get(format!(
            "https://api.brandfetch.io/v2/search/{}",
            merchant
                .website
                .as_ref()
                .and_then(|website| {
                    dbg!(&website);
                    let url = reqwest::Url::parse(website).ok()?;
                    url.domain().map(|d| d.to_string())
                })
                .unwrap_or(merchant.name.clone())
        ))
        .await?;
        dbg!(&request);
        let brands = request.text().await?;
        dbg!(&brands);
        let brands = serde_json::from_str::<Vec<BrandfetchResponse>>(&brands)?;
        let brand = brands.first();

        if let Some(brand) = brand {
            merchant.logo_url = brand.icon.clone();
            merchant.name = brand.name.clone().unwrap_or(merchant.name);
            merchant.external_id = Some(brand.brandId.clone());
        }

        Ok(merchant)
    }
}

#[cfg(test)]
mod tests {
    use std::env;

    use super::*;

    #[tokio::test]
    async fn test_add_brandfetch_with_valid_merchant() {
        // Arrange
        dotenvy::dotenv().ok();
        let enricher = Client::new(env::var("OPENAI_API_KEY").unwrap());

        let merchant = NewMerchant {
            name: "Example Merchant".to_string(),
            location: None,
            location_structured: None,
            labels: None,
            website: Some("https://apple.com".to_string()),
            logo_url: None,
            external_id: None,
        };

        // Act
        let result = enricher.add_brandfetch(merchant).await;

        // Assert
        assert!(result.is_ok());
        let enriched_merchant = result.unwrap();
        assert_eq!(enriched_merchant.name, "Apple");
        assert_eq!(enriched_merchant.website, Some("https://apple.com".to_string()));
        assert_ne!(enriched_merchant.logo_url, None);
        assert_ne!(enriched_merchant.external_id, None);
    }
}
