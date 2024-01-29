use actix_identity::Identity;
use actix_web::web;
use actix_web::{FromRequest, HttpMessage, HttpRequest};
use paperclip::actix::Apiv2Schema;
use paperclip::actix::{api_v2_operation, web::Json};
use serde::Deserialize;

use crate::models::{NewUser, UpdateUser, User};
use crate::server::{AppState, Error};
use crate::ultrafinance::verify_password;

#[api_v2_operation]
pub async fn get_me_endpoint(user: User) -> Result<Json<User>, Error> {
    Ok(Json(user))
}

#[api_v2_operation]
pub async fn create_user_endpoint(
    state: web::Data<AppState>,
    data: web::Json<NewUser>,
) -> Result<Json<User>, Error> {
    dbg!("Callwd create user endooint");
    let db = state.sqlx_pool.clone();

    let new_user = data.into_inner();
    let user = new_user.sqlx_create(&db).await?;
    Ok(Json(user))
}

#[derive(Deserialize, Apiv2Schema, ts_rs::TS)]
#[ts(export)]
pub struct CreateSession {
    pub email: String,
    pub password: String,
}

#[api_v2_operation]
pub async fn create_session_endpoint(
    state: web::Data<AppState>,
    data: web::Json<CreateSession>,
    request: HttpRequest,
) -> Result<Json<User>, Error> {
    let db = state.sqlx_pool.clone();
    let password = data.password.clone();
    let user = User::sqlx_by_email(&data.email, &db).await?;
    verify_password(&user.password, &password)?;
    Identity::login(&request.extensions(), user.id.to_string())?;
    Ok(Json(user))
}

#[api_v2_operation]
pub async fn delete_session_endpoint(request: HttpRequest) -> Result<Json<()>, Error> {
    let identity = Identity::extract(&request)
        .into_inner()
        .map_err(|e| -> Error { anyhow::anyhow!(e.to_string()).into() })?;
    identity.logout();

    Ok(Json(()))
}

#[api_v2_operation]
pub async fn update_me_endpoint(
    user: User,
    state: web::Data<AppState>,
    data: web::Json<UpdateUser>,
) -> Result<Json<User>, Error> {
    let db = state.sqlx_pool.clone();
    let mut update_user = data.into_inner();
    update_user.id = Some(user.id);
    let user = update_user.sqlx_update(&db).await?;
    Ok(Json(user))
}
