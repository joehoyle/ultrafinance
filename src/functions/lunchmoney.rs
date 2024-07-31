use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::{ultrafinance::TransactionDestination, FunctionParam, FunctionParams, Transaction};

#[derive(Serialize, Deserialize)]
struct Config {
    #[serde(rename = "apiKey")]
    api_key: String,
    #[serde(rename = "accountId")]
    account_id: String,
    #[serde(rename = "openaiApiKey")]
    openai_api_key: String,
}

#[derive(Deserialize, Debug)]
struct Category {
    name: String,
    id: u32,
}

#[derive(Deserialize, Debug)]
struct CategoriesResponse {
    categories: Vec<Category>,
}

#[derive(Deserialize, Debug)]
struct InsertTransactionResponse {
    ids: Option<Vec<u32>>,
    error: Option<Vec<String>>,
}

#[derive(Serialize, Debug)]
struct DraftTransaction {
    asset_id: u32,
    date: String,
    amount: String,
    currency: String,
    payee: String,
    notes: String,
    status: String,
    external_id: String,
    category_id: Option<u32>,
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

        let lm_categories: CategoriesResponse = client
            .get("https://dev.lunchmoney.app/v1/categories")
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .send()
            .await?
            .json()
            .await?;

        let categories: Vec<String> = lm_categories
            .categories
            .iter()
            .map(|cat| cat.name.clone())
            .collect();
        let chat_input = format!(
            "You are an assistant to categorize financial transactions. \
            You only respond with the exact category name and nothing else. The available categories are:\n\n{}",
            categories.join("\n")
        );

        let openai_client = async_openai::Client::new();
        let request = async_openai::types::CreateChatCompletionRequestArgs::default()
            .max_tokens(512u16)
            .model("gpt-4o")
            .messages([
                async_openai::types::ChatCompletionRequestSystemMessageArgs::default()
                    .content(chat_input)
                    .build()?
                    .into(),
                async_openai::types::ChatCompletionRequestUserMessageArgs::default()
                    .content(serde_json::to_string(&transaction)?)
                    .build()?
                    .into(),
            ])
            .build()?;

        let response = openai_client.chat().create(request).await?;
        let mut category_id = None;

        if let Some(choice) = response.choices.first() {
            let category_name = choice.message.content.as_ref().unwrap();
            if let Some(category) = lm_categories.categories.iter().find(|category| &category.name == category_name) {
                category_id = Some(category.id);
            }
        }
        let draft_transaction = DraftTransaction {
            category_id,
            asset_id: self.config.account_id.parse::<u32>()?,
            date: transaction.booking_date.to_string(),
            amount: transaction.transaction_amount.clone(),
            currency: transaction
                .transaction_amount_currency
                .to_string()
                .to_lowercase(),
            payee: transaction
                .creditor_name
                .clone()
                .or(transaction.debtor_name.clone())
                .unwrap_or_default(),
            notes: transaction
                .remittance_information
                .clone()
                .unwrap_or_default(),
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

        let inserted = client
            .post("https://dev.lunchmoney.app/v1/transactions")
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .json(&create_transactions_body)
            .send()
            .await?
            .text()
            .await?;

        let inserted = serde_json::from_str::<InsertTransactionResponse>(&inserted)?;

        if inserted.error.is_some() {
            return Err(anyhow::anyhow!(
                "Lunchmoney transactions failed to insert: {}",
                inserted.error.unwrap().join(", ")
            ));
        }
        Ok(())
    }
}
