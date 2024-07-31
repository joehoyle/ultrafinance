use actix_web::web;
use paperclip::actix::{api_v2_operation, web::Json};

use crate::exchange_rate::ExchangeRate;
use crate::models::User;
use crate::server::{AppState, Error};


#[api_v2_operation]
pub async fn get_exchange_rate(
    user: User,
    state: web::Data<AppState>,
) -> Result<Json<ExchangeRate>, Error> {
	// let exchange_rate = ExchangeRate::get_by_currency(&user.primary_currency, &state.sqlx_pool).await?;
	// Ok(Json(exchange_rate))
	Err(anyhow::anyhow!("Not implemented"))
}
