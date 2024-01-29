use crate::accounts::SourceTransaction;
use crate::utils::display_option;
use cli_table::Table;

use paperclip::actix::Apiv2Schema;
use serde::{Deserialize, Serialize};

use anyhow::Result;

#[derive(Table, Debug, Serialize, Deserialize, ts_rs::TS, Apiv2Schema)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
#[derive(sqlx::FromRow)]
pub struct Transaction {
    #[table(title = "Transaction ID")]
    pub id: u32,
    #[table(skip)]
    pub external_id: String,
    #[table(title = "Creditor Name", display_fn = "display_option")]
    pub creditor_name: Option<String>,
    #[table(title = "Debtor Name", display_fn = "display_option")]
    pub debtor_name: Option<String>,
    #[table(skip)]
    pub remittance_information: Option<String>,
    #[table(title = "Booking Date")]
    pub booking_date: chrono::NaiveDate,
    #[table(title = "Booking Datetime", display_fn = "display_option")]
    pub booking_datetime: Option<chrono::NaiveDateTime>,
    #[table(title = "Amount")]
    pub transaction_amount: String,
    #[table(title = "Currency")]
    pub transaction_amount_currency: String,
    #[table(
        title = "Proprietary Bank Transaction Code",
        display_fn = "display_option"
    )]
    pub proprietary_bank_transaction_code: Option<String>,
    #[table(title = "Exchange Rate", display_fn = "display_option")]
    pub currency_exchange_rate: Option<String>,
    #[table(title = "Exchange Source Currency", display_fn = "display_option")]
    pub currency_exchange_source_currency: Option<String>,
    #[table(title = "Exchange Target Currency", display_fn = "display_option")]
    pub currency_exchange_target_currency: Option<String>,
    #[table(skip)]
    pub merchant_id: Option<u32>,
    #[table(title = "Account ID")]
    pub account_id: u32,
    #[table(title = "User ID")]
    #[serde(skip_serializing)]
    pub user_id: u32,
    #[table(title = "Date Created")]
    pub created_at: chrono::NaiveDateTime,
    #[table(title = "Updated At")]
    pub updated_at: chrono::NaiveDateTime,
}

impl Transaction {
    pub async fn sqlx_all(db: &sqlx::MySqlPool) -> Result<Vec<Self>, anyhow::Error> {
        sqlx::QueryBuilder::new("SELECT * FROM transactions")
            .build_query_as::<Self>()
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_id(id: u32, db: &sqlx::MySqlPool) -> Result<Self, anyhow::Error> {
        sqlx::QueryBuilder::new("SELECT * FROM transactions WHERE id = ?")
            .push_bind(id)
            .build_query_as::<Self>()
            .fetch_one(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_id_by_user(
        id: u32,
        user_id: u32,
        db: &sqlx::MySqlPool,
    ) -> Result<Self, anyhow::Error> {
        sqlx::query_as!(
            Self,
            "SELECT * FROM transactions WHERE id = ? AND user_id = ?",
            id,
            user_id
        )
        .fetch_one(db)
        .await
        .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_delete(&self, db: &sqlx::MySqlPool) -> Result<(), anyhow::Error> {
        sqlx::query!("DELETE FROM transactions WHERE id = ?", &self.id)
            .execute(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(())
    }

    pub async fn sqlx_by_account(
        account_id: u32,
        db: &sqlx::MySqlPool,
    ) -> Result<Self, anyhow::Error> {
        sqlx::QueryBuilder::new("SELECT * FROM transactions WHERE account_id = ?")
            .push_bind(account_id)
            .build_query_as::<Self>()
            .fetch_one(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_user_by_search(
        user_id: u32,
        search: &str,
        db: &sqlx::MySqlPool,
    ) -> Result<Vec<Self>, anyhow::Error> {
        sqlx::query_as::<_, Self>(
            "
            SELECT * FROM transactions
            LEFT JOIN merchants ON transactions.merchant_id = merchants.id
            WHERE user_id = ?
            AND (creditor_name LIKE ? OR debtor_name LIKE ? OR remittance_information LIKE ? OR merchants.name LIKE ? OR merchants.labels LIKE ?)",
        )
        .bind(&user_id)
        .bind(format!("%{}%", search))
        .bind(format!("%{}%", search))
        .bind(format!("%{}%", search))
        .bind(format!("%{}%", search))
        .bind(format!("%{}%", search))
        .fetch_all(db)
        .await
        .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_without_merchant_liimt_100(
        db: &sqlx::MySqlPool,
    ) -> Result<Vec<Self>, anyhow::Error> {
        sqlx::query_as!(Self, "SELECT * FROM transactions WHERE merchant_id = NULL ORDER BY booking_date DESC LIMIT 100")
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_update(&mut self, db: &sqlx::MySqlPool) -> Result<Self, anyhow::Error> {
        self.updated_at = chrono::Local::now().naive_local();
        sqlx::query_as::<_, Self>("UPDATE transactions SET external_id = ?, creditor_name = ?, debtor_name = ?, remittance_information = ?, booking_date = ?, booking_datetime = ?, transaction_amount = ?, transaction_amount_currency = ?, proprietary_bank_transaction_code = ?, currency_exchange_rate = ?, currency_exchange_source_currency = ?, currency_exchange_target_currency = ?, merchant_id = ?, account_id = ?, user_id = ?, created_at = ?, updated_at = ? WHERE id = ?")
            .bind(&self.external_id)
            .bind(&self.creditor_name)
            .bind(&self.debtor_name)
            .bind(&self.remittance_information)
            .bind(&self.booking_date)
            .bind(&self.booking_datetime)
            .bind(&self.transaction_amount)
            .bind(&self.transaction_amount_currency)
            .bind(&self.proprietary_bank_transaction_code)
            .bind(&self.currency_exchange_rate)
            .bind(&self.currency_exchange_source_currency)
            .bind(&self.currency_exchange_target_currency)
            .bind(&self.merchant_id)
            .bind(&self.account_id)
            .bind(&self.user_id)
            .bind(&self.created_at)
            .bind(&self.updated_at)
            .bind(&self.id)
            .fetch_one(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }
}

pub struct NewTransaction {
    pub external_id: String,
    pub creditor_name: Option<String>,
    pub debtor_name: Option<String>,
    pub remittance_information: Option<String>,
    pub booking_date: chrono::NaiveDate,
    pub booking_datetime: Option<chrono::NaiveDateTime>,
    pub transaction_amount: String,
    pub transaction_amount_currency: String,
    pub proprietary_bank_transaction_code: Option<String>,
    pub currency_exchange_rate: Option<String>,
    pub currency_exchange_source_currency: Option<String>,
    pub currency_exchange_target_currency: Option<String>,
    pub account_id: u32,
    pub user_id: u32,
}

impl From<SourceTransaction> for NewTransaction {
    fn from(transaction: SourceTransaction) -> Self {
        Self {
            external_id: transaction.id,
            creditor_name: transaction.creditor_name,
            debtor_name: transaction.debtor_name,
            remittance_information: transaction.remittance_information,
            booking_date: transaction.booking_date,
            booking_datetime: transaction.booking_datetime,
            transaction_amount: transaction.transaction_amount.amount,
            transaction_amount_currency: transaction.transaction_amount.currency,
            proprietary_bank_transaction_code: transaction.proprietary_bank_transaction_code,
            currency_exchange_rate: transaction.currency_exchange_rate,
            currency_exchange_source_currency: transaction.currency_exchange_source_currency,
            currency_exchange_target_currency: transaction.currency_exchange_target_currency,
            account_id: 0,
            user_id: 0,
        }
    }
}
