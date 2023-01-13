use actix_identity::Identity;
use actix_web::web::{self, block};
use actix_web::{FromRequest, HttpMessage, HttpRequest};
use paperclip::actix::Apiv2Schema;
use paperclip::actix::{api_v2_operation, web::Json};
use serde::Deserialize;

use crate::models::{NewUser, UpdateUser, User};
use crate::ultrafinance::verify_password;
use crate::server::{AppState, Error};
use crate::schema;

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
    let db = state.db.clone();
    let user = block(move || -> Result<User, Error> {
        let mut con = db.get()?;
        let new_user = data.into_inner();
        new_user.create(&mut con).map_err(|e| e.into())
    })
    .await
    .unwrap();
    user.map(Json)
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
    let db = state.db.clone();
    let password = data.password.clone();
    let user = block(move || -> Result<User, Error> {
        let mut con = db.get()?;
        use diesel::*;
        use schema::users::dsl::*;
        User::all()
            .filter(email.eq(&data.email))
            .first(&mut con)
            .map_err(|e| -> Error { e.into() })
    })
    .await
    .unwrap()?;

    verify_password(&user.password, &password)?;

    Identity::login(&request.extensions(), user.id.to_string())?;
    Ok(Json(user))
}

#[api_v2_operation]
pub async fn delete_session_endpoint(
    request: HttpRequest,
) -> Result<Json<()>, Error> {
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
    dbg!("Callwd upsarew me endooint");
    let db = state.db.clone();
    let user = block(move || -> Result<User, Error> {
        let mut con = db.get()?;
        let mut update_user = data.into_inner();
        update_user.id = Some(user.id);
        update_user.update(&mut con)?;
        use diesel::*;
        User::by_id(user.id).first(&mut con).map_err(|e| e.into())
    })
    .await
    .unwrap();
    user.map(Json)
}
