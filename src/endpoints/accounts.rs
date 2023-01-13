use std::collections::HashMap;

use actix_web::web::block;
use anyhow::anyhow;
use paperclip::actix::Apiv2Schema;
use paperclip::actix::{api_v2_operation, web, web::Json};
use serde::Deserialize;

use crate::models::{Account, NewAccount, UpdateAccount, Transaction, User};
use crate::{nordigen, ultrafinance};
use crate::server::{AppState, Error};

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
    block(move || {
        let mut nordigen = nordigen::Nordigen::new();
        nordigen.populate_token()?;
        let requisition = nordigen.get_requisition(&data.requisition_id)?;
        let accounts = requisition
            .accounts
            .into_iter()
            .map(|account_id| {
                (
                    account_id.clone(),
                    nordigen.get_account_details(&account_id),
                )
            })
            .collect::<Vec<(String, anyhow::Result<nordigen::AccountDetails>)>>();

        let mut created_accounts: Vec<Account> = vec![];
        let db = state.db.clone();

        if accounts.is_empty() {
            return Err(anyhow!("No accounts found.").into());
        }

        let mut con = db.get()?;
        for (account_id, account) in accounts {
            match account {
                Ok(account) => {
                    let new_account = NewAccount::new("", &account_id, &account, &user)?;
                    let account = new_account.create(&mut *con);
                    match account {
                        Ok(account) => created_accounts.push(account),
                        Err(e) => return Err(e.into()),
                    }
                }
                Err(_e) => {}
            }
        }

        Ok(Json(created_accounts))
    })
    .await
    .unwrap()
}

#[api_v2_operation]
pub async fn sync_account_endpoint(
    user: User,
    state: web::Data<AppState>,
    path: web::Path<i32>,
) -> Result<Json<Vec<Transaction>>, Error> {
    let db = state.db.clone();
    let account_id: i32 = path.into_inner();

    let transactions = block(move || -> Result<Vec<Transaction>, Error> {
        use diesel::*;
        let mut con = db.get()?;
        let account: Result<Account, diesel::result::Error> = Account::by_id(account_id, user.id)
            .first(&mut con);
        crate::ultrafinance::import_transactions(&account?, &mut con).map_err(|e| e.into())
    })
    .await
    .unwrap();

    transactions.map(Json)
}

#[api_v2_operation]
pub async fn sync_accounts_endpoint(
    user: User,
    state: web::Data<AppState>,
) -> Result<Json<HashMap<i32, Result<Vec<Transaction>, Error>>>, Error> {
    let db = state.db.clone();

    let transactions = block(move || -> Result<HashMap<i32, Result<Vec<Transaction>, Error>>, Error> {
        use diesel::*;
        let mut con = db.get()?;
        let accounts = Account::by_user(&user).load(&mut con).map_err(|e| -> Error { e.into() })?;
        let transactions_map = ultrafinance::sync_accounts(&accounts, db);
        let mut transactions_map_response = HashMap::new();
        for (account_id, result) in transactions_map {
            transactions_map_response.insert(account_id, result.map_err(|e| -> Error { e.into() }));
        }

        Ok(transactions_map_response)
    })
    .await
    .unwrap();

    transactions.map(Json)
}

#[api_v2_operation]
pub async fn get_account_endpoint(
    user: User,
    state: web::Data<AppState>,
    path: web::Path<i32>,
) -> Result<Json<Account>, Error> {
    let db = state.db.clone();
    let account_id: i32 = path.into_inner();
    let account = block(move || -> Result<Account, Error> {
        use diesel::*;
        let mut con = db.get()?;
        Account::by_id(account_id, user.id)
            .first(&mut con)
            .map_err(|e| e.into())
    })
    .await
    .unwrap();

    account.map(Json)
}

#[api_v2_operation]
pub async fn update_account_endpoint(
    user: User,
    state: web::Data<AppState>,
    data: web::Json<UpdateAccount>,
    path: web::Path<i32>,
) -> Result<Json<Account>, Error> {
    let db = state.db.clone();
    let account_id: i32 = path.into_inner();
    let mut update_account = data.into_inner();
    update_account.id = Some(account_id);
    let account = block(move || -> Result<Account, Error> {
        use diesel::*;
        let mut con = db.get()?;
        // Validate
        Account::by_id(account_id, user.id).first(&mut con)?;
        update_account.update(&mut con).map_err(|e| e.into())
    })
    .await
    .unwrap();

    account.map(Json)
}

#[api_v2_operation]
pub async fn get_accounts_endpoint(
    user: User,
    state: web::Data<AppState>,
) -> Result<Json<Vec<Account>>, Error> {
    let db = state.db.clone();
    let accounts = block(move || -> Result<Vec<Account>, Error> {
        use diesel::*;
        let mut con = db.get()?;
        Account::by_user(&user).load(&mut con).map_err(|e| e.into())
    })
    .await
    .unwrap();

    accounts.map(Json)
}
