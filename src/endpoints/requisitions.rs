use paperclip::actix::Apiv2Schema;
use paperclip::actix::{api_v2_operation, web, web::Json};
use serde::Deserialize;

use crate::models::User;
use crate::server::{AppState, Error};
use crate::{nordigen, sqlx_create_nordigen_requisition, Account};

#[api_v2_operation]
pub async fn get_requisitions_institutions_endpoint(
) -> Result<Json<Vec<nordigen::Institution>>, Error> {
    let mut nordigen = nordigen::Nordigen::new();
    nordigen.populate_token().await?;
    let institutions = nordigen.get_institutions(&None).await;

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
    let db = state.sqlx_pool.clone();

    let requisition = if let Some(institution_id) = data.institution_id.clone() {
        nordigen.populate_token().await?;
        nordigen
            .create_requisition(
                &format!("{}{}", &state.url, "/accounts/resume"),
                &institution_id,
            )
            .await
    } else if let Some(account_id) = data.account_id {
        let account = Account::sqlx_by_id(account_id, user.id, &db).await?;
        nordigen.populate_token().await?;
        let nordigen_account = nordigen.get_account(&account.nordigen_id).await?;
        nordigen
            .create_requisition(
                &format!("{}/accounts/{}/resume", &state.url, account_id),
                &nordigen_account.institution_id,
            )
            .await
    } else {
        Err(anyhow::anyhow!("No institution_id or account_id provided."))
    };

    match requisition {
        Ok(requisition) => {
            let inserted_requisition =
                sqlx_create_nordigen_requisition(&requisition, &user, &db).await;
            match inserted_requisition {
                Ok(_inserted_requisition) => Ok(Json(requisition)),
                Err(e) => Err(e.into()),
            }
        }
        Err(e) => Err(e.into()),
    }
}
