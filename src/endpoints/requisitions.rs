use diesel::RunQueryDsl;
use paperclip::actix::Apiv2Schema;
use paperclip::actix::{api_v2_operation, web, web::Json};
use serde::Deserialize;

use crate::models::{NordigenRequisition, User};
use crate::{nordigen, Account};
use crate::schema;
use crate::ultrafinance::DbPool;
use crate::server::{AppState, Error};

#[api_v2_operation]
pub async fn get_requisitions_institutions_endpoint(
) -> Result<Json<Vec<nordigen::Institution>>, Error> {
    let mut nordigen = nordigen::Nordigen::new();
    let institutions: anyhow::Result<Vec<nordigen::Institution>> =
        actix_web::web::block(move || {
            nordigen.populate_token()?;
            let institutions = nordigen.get_institutions(&None)?;
            Ok(institutions)
        })
        .await
        .unwrap();

    match institutions {
        Ok(institutions) => Ok(Json(institutions)),
        Err(e) => Err(e.into()),
    }
}

#[derive(Deserialize, Apiv2Schema)]
pub struct CreateRequisition {
    institution_id: Option<String>,
    account_id: Option<u32>,
}

#[api_v2_operation]
pub async fn create_requisition_endpoint(
    user: User,
    state: web::Data<AppState>,
    data: web::Json<CreateRequisition>,
) -> Result<Json<nordigen::Requisition>, Error> {
    let mut nordigen = nordigen::Nordigen::new();
    let db = state.db.clone();

    let requisition = if let Some(institution_id) = data.institution_id.clone() {
        actix_web::web::block(move || {
            nordigen.populate_token()?;
            nordigen.create_requisition(
                &format!("{}{}", &state.url, "/accounts/resume"),
                &institution_id,
            )
        })
        .await
        .unwrap()
    } else if let Some(account_id) = data.account_id {
        let mut con = db.get().map_err(anyhow::Error::msg).unwrap();
        actix_web::web::block(move || {

            let account = Account::by_id(account_id, user.id).first(&mut con)?;
            nordigen.populate_token()?;
            let nordigen_account = nordigen.get_account(&account.nordigen_id)?;
            nordigen.create_requisition(&format!("{}/accounts/{}/resume", &state.url, account_id), &nordigen_account.institution_id)
        })
        .await
        .unwrap()
    } else {
        Err(anyhow::anyhow!("No institution_id or account_id provided."))
    };

    match requisition {
        Ok(requisition) => {
            let inserted_requisition = create_nordigen_requisition(&requisition, &user, &db).await;
            match inserted_requisition {
                Ok(_inserted_requisition) => Ok(Json(requisition)),
                Err(e) => Err(e.into()),
            }
        }
        Err(e) => Err(e.into()),
    }
}

pub async fn create_nordigen_requisition(
    requisition: &nordigen::Requisition,
    user: &User,
    db_pool: &DbPool,
) -> Result<NordigenRequisition, anyhow::Error> {
    let mut con = db_pool.get().map_err(anyhow::Error::msg).unwrap();
    let requisition = requisition.clone();
    let db_user_id = user.id;
    actix_web::web::block(move || {
        use diesel::*;
        use schema::nordigen_requisitions::dsl::*;
        match insert_into(nordigen_requisitions)
            .values((
                nordigen_id.eq(requisition.id.clone()),
                status.eq(requisition.status.clone()),
                user_id.eq(db_user_id),
            ))
            .execute(&mut con)
        {
            Ok(_) => {
                let requesition_id: u32 = select(schema::last_insert_id()).first(&mut con)?;
                let requesition: NordigenRequisition =
                    nordigen_requisitions.find(requesition_id).first(&mut con)?;
                Ok(requesition)
            }
            Err(e) => Err(anyhow::Error::msg(e.to_string())),
        }
    })
    .await?
}
