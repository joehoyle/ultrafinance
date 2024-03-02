use std::collections::HashMap;

use anyhow::anyhow;
use futures::stream::futures_unordered;
use futures::StreamExt;
use paperclip::actix::Apiv2Schema;
use paperclip::actix::{api_v2_operation, web, web::Json};
use serde::Deserialize;

use crate::accounts::SourceAccount;
use crate::models::{Account, NewAccount, Transaction, UpdateAccount, User};
use crate::server::{AppState, Error};
use crate::{nordigen, ultrafinance};

use super::transactions::{sqlx_add_merchants, TransactionWithMerchant};

#[derive(Deserialize, Apiv2Schema)]
pub struct CreateAccounts {
    requisition_id: String,
}

#[api_v2_operation]
pub async fn create_accounts_endpoint(
    user: User,
    state: web::Data<AppState>,
    data: web::Json<CreateAccounts>,
) -> Result<Json<Vec<Account>>, Error> {
    let mut nordigen = nordigen::Nordigen::new();
    nordigen.populate_token().await?;
    let requisition = nordigen.get_requisition(&data.requisition_id).await?;

    let mut accounts = vec![];
    for account_id in requisition.accounts {
        accounts.push((account_id.clone(), nordigen.get_account(&account_id).await));
    }

    let mut created_accounts: Vec<Account> = vec![];
    let db = state.sqlx_pool.clone();

    if accounts.is_empty() {
        return Err(anyhow!("No accounts found.").into());
    }

    for (_, account) in accounts {
        match account {
            Ok(account) => {
                let mut new_account = NewAccount::from(account.details().await?);
                new_account.user_id = user.id;
                new_account.config = serde_json::to_string(&account).ok();

                let account = new_account.sqlx_create(&db).await;
                match account {
                    Ok(account) => created_accounts.push(account),
                    Err(e) => return Err(e.into()),
                }
            }
            Err(_e) => {}
        }
    }

    Ok(Json(created_accounts))
}

#[api_v2_operation]
pub async fn sync_account_endpoint(
    user: User,
    state: web::Data<AppState>,
    path: web::Path<u32>,
) -> Result<Json<Vec<Transaction>>, Error> {
    let db = state.sqlx_pool.clone();
    let account_id: u32 = path.into_inner();
    let mut account = Account::sqlx_by_id(account_id, user.id, &db).await?;
    account.update_balance().await?;
    account.sqlx_update(&db).await?;
    let transactions = crate::ultrafinance::sqlx_import_transactions(&account, &db)
        .await
        .map_err(|e| e.into());
    transactions.map(Json)
}

#[api_v2_operation]
pub async fn sync_accounts_endpoint(
    user: User,
    state: web::Data<AppState>,
) -> Result<Json<HashMap<u32, Result<Vec<TransactionWithMerchant>, Error>>>, Error> {
    let db = &state.sqlx_pool;
    let accounts = Account::sqlx_by_user(user.id, &db).await?;

    let futures = futures_unordered::FuturesUnordered::new();

    for mut account in accounts {
        futures.push(async {
            let _ = account.update_balance().await;
            let _ = account.sqlx_update(db).await;
            account
        });
    }

    let mut accounts = futures.collect::<Vec<_>>().await;

    let transactions_map = ultrafinance::sqlx_sync_accounts(&mut accounts, db).await;
    let mut transactions_map_response = HashMap::new();
    for (account_id, result) in transactions_map {
        // Call add_merchants on transactions_map's value
        let result = match result {
            Ok(t) => Ok(sqlx_add_merchants(t, db).await),
            Err(e) => Err(e),
        };

        transactions_map_response.insert(account_id, result.map_err(|e| -> Error { e.into() }));
    }

    Ok(Json(transactions_map_response))
}

#[api_v2_operation]
pub async fn get_account_endpoint(
    user: User,
    state: web::Data<AppState>,
    path: web::Path<u32>,
) -> Result<Json<Account>, Error> {
    let db = &state.sqlx_pool;
    let account_id: u32 = path.into_inner();
    Account::sqlx_by_id(account_id, user.id, db)
        .await
        .map(Json)
        .map_err(|e| e.into())
}

#[api_v2_operation]
pub async fn update_account_endpoint(
    user: User,
    state: web::Data<AppState>,
    data: web::Json<UpdateAccount>,
    path: web::Path<u32>,
) -> Result<Json<Account>, Error> {
    let db = &state.sqlx_pool;
    let account_id: u32 = path.into_inner();
    let mut update_account = data.into_inner();
    update_account.id = Some(account_id);
    // Validate
    Account::sqlx_by_id(account_id, user.id, db)
        .await
        .map_err(|e| <anyhow::Error as Into<Error>>::into(e))?;
    let account = update_account.sqlx_update(db).await;

    account.map(Json).map_err(|e| e.into())
}

#[api_v2_operation]
pub async fn delete_account_endpoint(
    user: User,
    state: web::Data<AppState>,
    path: web::Path<u32>,
) -> Result<Json<()>, Error> {
    let db = &state.sqlx_pool;
    let account_id: u32 = path.into_inner();
    let account = Account::sqlx_by_id(account_id, user.id, db)
        .await
        .map_err(|e| <anyhow::Error as Into<Error>>::into(e))?;
    account
        .sqlx_delete(db)
        .await
        .map(Json)
        .map_err(|e| e.into())
}

#[derive(Deserialize, Apiv2Schema)]
pub struct RelinkAccount {
    requisition_id: String,
}

#[api_v2_operation]
pub async fn relink_account_endpoint(
    user: User,
    state: web::Data<AppState>,
    data: web::Json<RelinkAccount>,
    path: web::Path<u32>,
) -> Result<Json<Account>, Error> {
    let account_id: u32 = path.into_inner();
    let account = Account::sqlx_by_id(account_id, user.id, &state.sqlx_pool).await?;
    let db = &state.sqlx_pool;
    let mut accounts = vec![];
    let mut nordigen = nordigen::Nordigen::new();
    nordigen.populate_token().await?;
    let requisition = nordigen.get_requisition(&data.requisition_id).await?;

    for account_id in requisition.accounts {
        let nordigen_account = nordigen.get_account(&account_id).await?;
        let details = nordigen_account.details().await?;
        accounts.push((nordigen_account, details));
    }

    let mut account_to_return = Err(anyhow!("No account found.").into());

    for (nordigen_account, details) in accounts {
        let select_account =
            Account::sqlx_by_source_account_details(details, account.user_id, db).await;
        let mut account = match select_account {
            Ok(a) => a,
            Err(e) => {
                println!("Error in relinking: {}.", e);
                continue;
            }
        };
        account.config = serde_json::to_string(&nordigen_account).ok();
        account.sqlx_update(db).await?;
        account_to_return = Ok(account);
    }

    account_to_return.map(Json)
}

#[api_v2_operation]
pub async fn get_accounts_endpoint(
    user: User,
    state: web::Data<AppState>,
) -> Result<Json<Vec<Account>>, Error> {
    let accounts = Account::sqlx_by_user(user.id, &state.sqlx_pool).await;
    accounts.map(Json).map_err(|e| e.into())
}
