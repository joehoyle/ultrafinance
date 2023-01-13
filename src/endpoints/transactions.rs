use actix_web::web::block;
use paperclip::actix::web;
use paperclip::actix::{api_v2_operation, web::Json};

use crate::models::{Transaction, User};
use crate::schema;
use crate::ultrafinance::DbPool;
use crate::server::{AppState, Error};

#[api_v2_operation]
pub async fn get_transactions_endpoint(
    user: User,
    state: web::Data<AppState>,
) -> Result<Json<Vec<Transaction>>, Error> {
    let db = state.db.clone();
    let transactions = block(move || -> Result<Vec<Transaction>, Error> {
        let mut con = db.get()?;
        use diesel::*;
        Transaction::by_user(&user).load(&mut con).map_err(|e| e.into())
    })
    .await
    .unwrap();

    transactions.map(Json)
}

#[api_v2_operation]
pub async fn get_transaction_endpoint(
    user: User,
    state: web::Data<AppState>,
    path: web::Path<i32>,
) -> Result<Json<Transaction>, Error> {
    let db = state.db.clone();
    let transaction_id: i32 = path.into_inner();
    let transaction = get_transaction_for_user(transaction_id, &user, &db).await;
    match transaction {
        Ok(transaction) => Ok(Json(transaction)),
        Err(e) => Err(e.into()),
    }
}

pub async fn get_transaction_for_user(
    transaction_id: i32,
    user: &User,
    db_pool: &DbPool,
) -> Result<Transaction, anyhow::Error> {
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
}

#[api_v2_operation]
pub async fn delete_transaction_endpoint(
    user: User,
    state: web::Data<AppState>,
    path: web::Path<i32>,
) -> Result<Json<()>, Error> {
    let db = state.db.clone();
    let transaction_id: i32 = path.into_inner();
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
