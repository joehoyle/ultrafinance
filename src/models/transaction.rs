use crate::accounts::SourceTransaction;
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
use crate::models::Merchant;

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
    AsChangeset,
)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
#[diesel(belongs_to(User))]
#[diesel(belongs_to(Account))]
#[diesel(belongs_to(Merchant))]
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
    #[table(skip)]
    pub merchant_id: Option<i32>,
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
type JoinSqlType = diesel::dsl::SqlTypeOf<diesel::dsl::InnerJoin<transactions::table, merchants::table>>;
type BoxedQuery<'a> = crate::schema::transactions::BoxedQuery<'a, Mysql, SqlType>;
type JoinBoxedQuery<'a> = crate::schema::transactions::BoxedQuery<'a, Mysql, JoinSqlType>;

type TransactionJoinMerchant = diesel::dsl::InnerJoin<transactions::table, merchants::table>;

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

    // pub fn search(query: &str, user_id: i32) -> BoxedQuery<'static> {
    //     schema::transactions::table
    //         .inner_join(schema::merchants::table)
    //         .into_boxed()
    //         .select(Self::as_select())
    //         .filter(schema::transactions::user_id.eq(user_id))
    //         .filter(
    //             schema::transactions::creditor_name
    //                 .like(format!("%{}%", query))
    //                 .or(schema::transactions::debtor_name.like(format!("%{}%", query)))
    //                 .or(schema::transactions::creditor_name.like(format!("%{}%", query)))
    //                 .or(schema::transactions::remittance_information.like(format!("%{}%", query)))
    //                 .or(schema::merchants::name.like(format!("%{}%", query))),
    //         )

    // }

    pub fn by_id_only(id: i32) -> BoxedQuery<'static> {
        Self::all()
            .filter(schema::transactions::id.eq(id))
    }

    pub fn update(&mut self, con: &mut MysqlConnection) -> Result<()> {
        self.updated_at = chrono::Local::now().naive_local();
        diesel::update(&*self).set(&*self).execute(con)?;
        Ok(())
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
