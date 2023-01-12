use crate::schema::{*, self};
use crate::utils::display_option;
use cli_table::Table;

use diesel::*;
use diesel::mysql::Mysql;
use diesel::{Associations, Identifiable, Queryable};
use paperclip::actix::Apiv2Schema;
use serde::{Deserialize, Serialize};

use anyhow::Result;

use crate::models::Account;
use crate::models::User;

#[derive(
    Table,
    Identifiable,
    Queryable,
    Associations,
    Debug,
    Serialize,
    Deserialize,
    ts_rs::TS,
    Apiv2Schema,
    Selectable,
)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
#[diesel(belongs_to(User))]
#[diesel(belongs_to(Account))]
pub struct Transaction {
    #[table(title = "Transaction ID")]
    pub id: i32,
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
    proprietary_bank_transaction_code: Option<String>,
    #[table(title = "Exchange Rate", display_fn = "display_option")]
    currency_exchange_rate: Option<String>,
    #[table(title = "Exchange Source Currency", display_fn = "display_option")]
    currency_exchange_source_currency: Option<String>,
    #[table(title = "Exchange Target Currency", display_fn = "display_option")]
    currency_exchange_target_currency: Option<String>,
    #[table(title = "Account ID")]
    pub account_id: i32,
    #[table(title = "User ID")]
    #[serde(skip_serializing)]
    pub user_id: i32,
    #[table(title = "Date Created")]
    pub created_at: chrono::NaiveDateTime,
    #[table(title = "Updated At")]
    pub updated_at: chrono::NaiveDateTime,
}

type SqlType = diesel::dsl::SqlTypeOf<diesel::dsl::AsSelect<Transaction, Mysql>>;
type BoxedQuery<'a> = crate::schema::transactions::BoxedQuery<'a, Mysql, SqlType>;

impl Transaction {
    pub fn all() -> BoxedQuery<'static> {
        schema::transactions::table
            .select(Self::as_select())
            .into_boxed()
    }

    pub fn by_user(user: &User) -> BoxedQuery<'static> {
        Self::all().filter(schema::transactions::user_id.eq(user.id))
    }

    pub fn by_id(id: i32, user_id: i32) -> BoxedQuery<'static> {
        Self::all()
            .filter(schema::transactions::id.eq(id))
            .filter(schema::transactions::user_id.eq(user_id))
    }

    pub fn delete(self, con: &mut MysqlConnection) -> Result<()> {
        diesel::delete(&self).execute(con)?;
        Ok(())
    }
}

#[derive(Insertable)]
#[diesel(table_name = transactions)]
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
    pub account_id: i32,
    pub user_id: i32,
}
