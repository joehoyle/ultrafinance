use crate::{NewMerchant, Transaction};
use async_openai::{
    config::OpenAIConfig,
    types::{
        ChatCompletionFunctionsArgs, ChatCompletionRequestFunctionMessageArgs,
        ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs, Role,
    },
};
use deno_ast::swc::common::util::take::Take;
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
            .get(0).ok_or(anyhow::anyhow!("No choices in response"))?
            .message
            .clone();
        dbg!(&response_message);
        #[allow(deprecated)]
        if let Some(function_call) = response_message.function_call {
            // let function_name = function_call.name;
            // let function_args: serde_json::Value = function_call.arguments.parse().unwrap();
            let mut transactions_map = HashMap::new();
            let transactions = serde_json::from_str::<EnrichTransactionsArguments>(&function_call.arguments)?;
            for transaction in transactions.enriched_transactions {
                transactions_map.insert(transaction.transaction_id, transaction.merchant);
            }
            Ok(transactions_map)
        } else {
            Err(anyhow::anyhow!("No tool calls in response"))
        }
    }
}
