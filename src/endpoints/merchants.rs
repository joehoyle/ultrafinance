use paperclip::actix::web;
use paperclip::actix::{api_v2_operation, web::Json};

use crate::models::Merchant;
use crate::server::{AppState, Error};

#[api_v2_operation]
pub async fn get_merchant_endpoint(
    state: web::Data<AppState>,
    path: web::Path<u32>,
) -> Result<Json<Merchant>, Error> {
    let db = &state.sqlx_pool;
    let merchant_id: u32 = path.into_inner();
    let merchant = Merchant::sqlx_by_id(merchant_id, &db).await?;
    Ok(Json(merchant))
}
