use actix_web::web::{block, Query};
use paperclip::actix::{api_v2_operation, web::Json};
use paperclip::actix::{web, Apiv2Schema};
use serde::{Deserialize, Serialize};

use crate::models::{Merchant, Transaction, User};
use crate::server::{AppState, Error};
use crate::ultrafinance::DbPool;
use crate::{schema, transaction};

#[derive(Serialize, Apiv2Schema, ts_rs::TS)]
#[ts(export)]
pub struct TransactionWithMerchant {
    #[serde(flatten)]
    pub transaction: Transaction,
    pub merchant: Option<Merchant>,
}

#[derive(Apiv2Schema, Deserialize)]
pub struct PaginationArgs {
    pub page: Option<u64>,
    pub per_page: Option<u64>,
    pub search: Option<String>,
}

#[api_v2_operation]
pub async fn get_transactions_endpoint(
    user: User,
    state: web::Data<AppState>,
    query: Query<PaginationArgs>,
) -> Result<Json<Vec<TransactionWithMerchant>>, Error> {
    let db = state.db.clone();
    let transactions = block(move || -> Result<Vec<TransactionWithMerchant>, Error> {
        let mut con = db.get()?;
        let page = query.page.unwrap_or(1).max(1);
        let per_page = query.per_page.unwrap_or(100).max(1).min(1000);
        use diesel::*;
        let transactions = Transaction::by_user(&user)
            .left_join(schema::merchants::table)
            .offset(((page - 1) * per_page) as i64)
            .limit(per_page as _)
            .order(schema::transactions::id.desc());

        let transactions = match &query.search {
            Some(search) => transactions.filter(
                schema::transactions::creditor_name
                    .like(format!("%{}%", search))
                    .or(schema::transactions::debtor_name.like(format!("%{}%", search)))
                    .or(schema::transactions::creditor_name.like(format!("%{}%", search)))
                    .or(schema::transactions::remittance_information.like(format!("%{}%", search)))
                    .or(schema::merchants::name.like(format!("%{}%", search)))
                    .or(schema::merchants::labels.like(format!("%{}%", search))),
            ),
            None => transactions,
        };

        let transactions = transactions.load(&mut con).map_err(Error::from)?;
        Ok(add_merchants(transactions, &db))
    })
    .await
    .unwrap();

    transactions.map(Json)
}

#[api_v2_operation]
pub async fn get_transaction_endpoint(
    user: User,
    state: web::Data<AppState>,
    path: web::Path<u32>,
) -> Result<Json<TransactionWithMerchant>, Error> {
    let db = state.db.clone();
    let transaction_id: u32 = path.into_inner();
    let transaction = get_transaction_for_user(transaction_id, &user, &db).await;
    match transaction {
        Ok(transaction) => Ok(Json(transaction)),
        Err(e) => Err(e.into()),
    }
}

pub async fn get_transaction_for_user(
    transaction_id: u32,
    user: &User,
    db_pool: &DbPool,
) -> Result<TransactionWithMerchant, anyhow::Error> {
    let mut con = db_pool.get().map_err(anyhow::Error::msg).unwrap();
    let db_user_id = user.id;
    block(move || {
        use diesel::*;
        use schema::transactions::dsl::*;
        let transaction: Transaction = transactions
            .filter(user_id.eq(db_user_id))
            .filter(id.eq(transaction_id))
            .first(&mut con)?;
        Ok(transaction)
    })
    .await?
    .map(|transaction| {
        let transaction = add_merchant(transaction, &db_pool);
        transaction
    })
}

#[api_v2_operation]
pub async fn delete_transaction_endpoint(
    user: User,
    state: web::Data<AppState>,
    path: web::Path<u32>,
) -> Result<Json<()>, Error> {
    let db = state.db.clone();
    let transaction_id: u32 = path.into_inner();
    let transaction = block(move || -> Result<(), Error> {
        let mut con = db.get()?;
        use diesel::*;
        let transaction = Transaction::by_id(transaction_id, user.id).first(&mut con)?;
        transaction.delete(&mut con).map_err(|e| e.into())
    })
    .await
    .unwrap();

    transaction.map(Json)
}

pub fn add_merchants(
    transactions: Vec<Transaction>,
    db_pool: &DbPool,
) -> Vec<TransactionWithMerchant> {
    let transactions: Vec<TransactionWithMerchant> = transactions
        .into_iter()
        .map(|transaction| add_merchant(transaction, &db_pool))
        .collect();
    transactions
}

pub fn add_merchant(transaction: Transaction, db_pool: &DbPool) -> TransactionWithMerchant {
    let mut con = db_pool.get().map_err(anyhow::Error::msg).unwrap();
    let transaction: TransactionWithMerchant = TransactionWithMerchant {
        merchant: match &transaction.merchant_id {
            Some(merchant_id) => {
                use diesel::*;
                Merchant::by_id(*merchant_id).first(&mut con).ok()
            }
            None => None,
        },
        transaction,
    };
    transaction
}
