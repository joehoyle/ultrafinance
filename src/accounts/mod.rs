use std::fmt::Display;

use crate::{nordigen, utils::display_option};
use chrono::NaiveDate;
use cli_table::Table;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug, Serialize, Clone)]
pub struct Amount {
    pub amount: String,
    pub currency: String,
}

impl Display for Amount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.amount, self.currency)
    }
}

#[derive(Debug)]
pub struct SourceAccountDetails {
    pub id: String,
    pub number: String,
    pub currency: String,
    pub details: String,
    pub owner_name: Option<String>,
    pub icon: Option<String>,
    pub institution_name: String,
}

#[derive(Debug, Serialize, Table)]
pub struct SourceTransaction {
    pub id: String,
    #[table(display_fn = "display_option")]
    pub creditor_name: Option<String>,
    #[table(display_fn = "display_option")]
    pub debtor_name: Option<String>,
    #[table(display_fn = "display_option")]
    pub remittance_information: Option<String>,
    pub booking_date: chrono::NaiveDate,
    #[table(display_fn = "display_option")]
    pub booking_datetime: Option<chrono::NaiveDateTime>,
    pub transaction_amount: Amount,
    #[table(display_fn = "display_option")]
    pub currency_exchange_rate: Option<String>,
    #[table(display_fn = "display_option")]
    pub proprietary_bank_transaction_code: Option<String>,
    #[table(display_fn = "display_option")]
    pub currency_exchange_source_currency: Option<String>,
    #[table(display_fn = "display_option")]
    pub currency_exchange_target_currency: Option<String>,
}

pub trait SourceAccount {
    fn balance(&self) -> impl std::future::Future<Output = Result<Amount, anyhow::Error>> + Send;
    fn transactions(
        &self,
        date_from: &Option<NaiveDate>,
        date_to: &Option<NaiveDate>,
    ) -> impl std::future::Future<Output = Result<Vec<SourceTransaction>, anyhow::Error>> + Send;
    fn details(&self) -> impl std::future::Future<Output = Result<SourceAccountDetails, anyhow::Error>> + Send;
}

pub fn get_source_account(type_: &str, config: &str) -> Option<Box<impl SourceAccount>> {
    match type_ {
        "nordigen" => Some(Box::new(
            serde_json::from_str::<nordigen::Account>(config).unwrap(),
        )),
        _ => None,
    }
}
