use actix_web::web::Query;
use paperclip::actix::{api_v2_operation, web::Json};
use paperclip::actix::{web, Apiv2Schema};
use serde::{Deserialize, Serialize};

use crate::models::{Merchant, Transaction, User};
use crate::server::{AppState, Error};
use futures::stream::FuturesUnordered;
use futures::StreamExt;

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
    let db = state.sqlx_pool.clone();
    let transactions = Transaction::sqlx_by_user_by_search(
        user.id,
        &query.search.as_ref().unwrap_or(&"".to_string()),
        &db,
    )
    .await;
    let transactions = transactions?;
    Ok(Json(sqlx_add_merchants(transactions, &db).await))
}

#[api_v2_operation]
pub async fn get_transaction_endpoint(
    user: User,
    state: web::Data<AppState>,
    path: web::Path<u32>,
) -> Result<Json<TransactionWithMerchant>, Error> {
    let db = state.sqlx_pool.clone();
    let transaction_id: u32 = path.into_inner();
    let transaction = Transaction::sqlx_by_id_by_user(transaction_id, user.id, &db).await?;
    let transaction = sqlx_add_merchant(transaction, &db).await;
    Ok(Json(transaction))
}

#[api_v2_operation]
pub async fn delete_transaction_endpoint(
    user: User,
    state: web::Data<AppState>,
    path: web::Path<u32>,
) -> Result<Json<()>, Error> {
    let db = state.sqlx_pool.clone();

    let transaction_id: u32 = path.into_inner();
    let transaction = Transaction::sqlx_by_id_by_user(transaction_id, user.id, &db).await?;
    transaction.sqlx_delete(&db).await?;
    Ok(Json(()))
}


pub async fn sqlx_add_merchants(
    transactions: Vec<Transaction>,
    db_pool: &sqlx::MySqlPool,
) -> Vec<TransactionWithMerchant> {
    let mut futures = FuturesUnordered::new();

    for transaction in transactions {
        futures.push(sqlx_add_merchant(transaction, &db_pool));
    }

    let mut results = Vec::new();
    while let Some(result) = futures.next().await {
        results.push(result);
    }

    results
}

pub async fn sqlx_add_merchant(
    transaction: Transaction,
    db_pool: &sqlx::MySqlPool,
) -> TransactionWithMerchant {
    let transaction: TransactionWithMerchant = TransactionWithMerchant {
        merchant: match &transaction.merchant_id {
            Some(merchant_id) => Merchant::sqlx_by_id(*merchant_id, db_pool).await.ok(),
            None => None,
        },
        transaction,
    };
    transaction
}
