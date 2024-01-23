use actix_web::web::block;
use paperclip::actix::web;
use paperclip::actix::{api_v2_operation, web::Json};

use crate::models::Merchant;
use crate::server::{AppState, Error};

#[api_v2_operation]
pub async fn get_merchant_endpoint(
    state: web::Data<AppState>,
    path: web::Path<u32>,
) -> Result<Json<Merchant>, Error> {
    let db = state.db.clone();
    let merchant_id: u32 = path.into_inner();
    use diesel::*;
    let merchant =
        block(move || -> Result<Merchant, Error> {
            let mut con = db.get()?;
            Merchant::by_id(merchant_id).first(&mut con).map_err(|e| e.into())
        } )
        .await
        .unwrap();
    match merchant {
        Ok(merchant) => Ok(Json(merchant)),
        Err(e) => Err(e.into()),
    }
}
