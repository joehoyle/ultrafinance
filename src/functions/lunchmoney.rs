use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::{ultrafinance::TransactionDestination, FunctionParam, FunctionParams, Transaction};

#[derive(Serialize, Deserialize)]
struct Config {
    api_key: String,
    account_id: String,
    openai_api_key: String,
}

#[derive(Deserialize)]
struct Category {
    name: String,
}

#[derive(Serialize)]
struct DraftTransaction {
    asset_id: i32,
    date: String,
    amount: String,
    currency: String,
    payee: String,
    notes: String,
    status: String,
    external_id: String,
}

pub async fn get_params() -> Result<FunctionParams, anyhow::Error> {
    return Ok(FunctionParams::from([
        (
            "api_key".to_string(),
            FunctionParam {
                name: "Lunchmoney API Key".to_string(),
                r#type: "string".to_string(),
            },
        ),
        (
            "account_id".to_string(),
            FunctionParam {
                name: "Lunchmoney Account Id".to_string(),
                r#type: "string".to_string(),
            },
        ),
        (
            "openai_api_key".to_string(),
            FunctionParam {
                name: "OpenAI API Key".to_string(),
                r#type: "string".to_string(),
            },
        ),
    ]));
}

pub struct Lunchmoney {
    config: Config,
}

impl Lunchmoney {
    pub fn new(config: &str) -> Result<Self, anyhow::Error> {
        Ok(Self {
            config: serde_json::from_str(config)?,
        })
    }
}

#[async_trait]
impl TransactionDestination for Lunchmoney {
    async fn transaction_created(&self, transaction: &Transaction) -> Result<(), anyhow::Error> {
        let client = Client::new();

        let lm_categories: Vec<Category> = client
            .get("https://dev.lunchmoney.app/v1/categories")
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .send()
            .await?
            .json()
            .await?;

        let categories: Vec<String> = lm_categories.iter().map(|cat| cat.name.clone()).collect();
        let chat_input = format!(
            "You are an assistant to categorize financial transactions. \
            You only respond with the exact category name and nothing else. The available categories are:\n\n{}",
            categories.join("\n")
        );

        let chat_body = serde_json::json!({
            "messages": [
                { "role": "system", "text": chat_input },
                { "role": "user", "text": serde_json::to_string(&transaction)? }
            ],
            "model": "gpt-4-turbo"
        });

        let chat_response: Vec<serde_json::Value> = client
            .post("https://api.openai.com/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.config.openai_api_key))
            .json::<serde_json::Value>(&chat_body)
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?
            .get("choices")
            .and_then(|choices| choices.as_array().cloned())
            .unwrap_or_default();

        println!("{:?}", chat_response);

        let draft_transaction = DraftTransaction {
            asset_id: self.config.account_id.parse::<i32>()?,
            date: transaction.booking_date.to_string(),
            amount: transaction.transaction_amount.clone(),
            currency: transaction.transaction_amount_currency.to_string(),
            payee: transaction
                .creditor_name.clone()
                .or(transaction.debtor_name.clone())
                .unwrap_or_default(),
            notes: transaction.remittance_information.clone().unwrap_or_default(),
            status: String::from("cleared"),
            external_id: transaction.external_id.clone(),
        };

        let create_transactions_body = serde_json::json!({
            "transactions": [draft_transaction],
            "apply_rules": true,
            "check_for_recurring": true,
            "debit_as_negative": true,
            "skip_balance_update": false
        });

        client
            .post("https://dev.lunchmoney.app/v1/transactions")
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .json(&create_transactions_body)
            .send()
            .await?;

        Ok(())
    }
}
