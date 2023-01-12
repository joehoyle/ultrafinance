use paperclip::actix::Apiv2Schema;
use paperclip::actix::{api_v2_operation, web, web::Json};
use serde::Deserialize;

use crate::models::{NordigenRequisition, User};
use crate::nordigen;
use crate::schema;
use crate::ultrafinance::DbPool;
use crate::{AppState, Error};

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
    institution_id: String,
}

#[api_v2_operation]
pub async fn create_requisition_endpoint(
    user: User,
    state: web::Data<AppState>,
    data: web::Json<CreateRequisition>,
) -> Result<Json<nordigen::Requisition>, Error> {
    let mut nordigen = nordigen::Nordigen::new();
    let db = state.db.clone();
    let requisition: anyhow::Result<nordigen::Requisition> = actix_web::web::block(move || {
        nordigen.populate_token()?;
        let requisition = nordigen.create_requisition(
            &format!("{}{}", &state.url, "/accounts/resume"),
            &data.institution_id,
        )?;
        //
        Ok(requisition)
    })
    .await
    .unwrap();

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
                let requesition_id: i32 = select(schema::last_insert_id()).first(&mut con)?;
                let requesition: NordigenRequisition =
                    nordigen_requisitions.find(requesition_id).first(&mut con)?;
                Ok(requesition)
            }
            Err(e) => Err(anyhow::Error::msg(e.to_string())),
        }
    })
    .await?
}
