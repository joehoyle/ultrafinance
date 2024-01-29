use std::collections::HashMap;

use actix_web::web;
use paperclip::actix::Apiv2Schema;
use paperclip::actix::{api_v2_operation, web::Json};
use serde::Deserialize;

use crate::models::{Function, NewTrigger, Trigger, TriggerFilter, TriggerLog, TriggerQueue, User};
use crate::server::{AppState, Error};
use crate::ultrafinance;

#[derive(Apiv2Schema)]
#[allow(dead_code)]
pub struct GetTriggersQuery {
    function_id: Option<u32>,
}

#[api_v2_operation]
pub async fn get_triggers_endpoint(
    user: User,
    state: web::Data<AppState>,
) -> Result<Json<Vec<Trigger>>, Error> {
    let db = state.sqlx_pool.clone();
    let triggers = Trigger::sqlx_by_user(user.id, &db).await?;
    Ok(Json(triggers))
}

#[api_v2_operation]
pub async fn get_trigger_endpoint(
    user: User,
    state: web::Data<AppState>,
    path: web::Path<u32>,
) -> Result<Json<Trigger>, Error> {
    let db = state.sqlx_pool.clone();
    let trigger_id: u32 = path.into_inner();
    let trigger = Trigger::sqlx_by_id_by_user(trigger_id, user.id, &db).await?;
    Ok(Json(trigger))
}

#[derive(Deserialize, Apiv2Schema, ts_rs::TS)]
#[ts(export)]
pub struct CreateTrigger {
    function_id: u32,
    params: HashMap<String, String>,
    event: String,
    name: String,
}

#[api_v2_operation]
pub async fn create_trigger_endpoint(
    user: User,
    state: web::Data<AppState>,
    data: web::Json<CreateTrigger>,
) -> Result<Json<Trigger>, Error> {
    let db = state.sqlx_pool.clone();

    let function = Function::sqlx_by_id_by_user(data.function_id, user.id, &db).await?;
    let new_trigger = NewTrigger {
        event: data.event.clone(),
        name: data.name.clone(),
        filter: TriggerFilter(vec![]), // todo
        params: serde_json::to_string(&data.params)?,
        user_id: user.id,
        function_id: function.id,
    };

    let trigger = new_trigger.sqlx_create(&db).await?;
    Ok(Json(trigger))
}

#[derive(Deserialize, Apiv2Schema, ts_rs::TS)]
#[ts(export)]
pub struct UpdateTrigger {
    function_id: Option<u32>,
    params: Option<HashMap<String, String>>,
    event: Option<String>,
    name: Option<String>,
}

impl From<UpdateTrigger> for crate::models::UpdateTrigger {
    fn from(update: UpdateTrigger) -> Self {
        crate::models::UpdateTrigger {
            id: None,
            name: update.name,
            params: match update.params {
                Some(params) => serde_json::to_string(&params).ok(),
                None => None,
            },
            event: update.event,
            filter: None,
            function_id: update.function_id,
        }
    }
}

#[api_v2_operation]
pub async fn update_trigger_endpoint(
    user: User,
    state: web::Data<AppState>,
    data: web::Json<UpdateTrigger>,
    path: web::Path<u32>,
) -> Result<Json<Trigger>, Error> {
    let db = state.sqlx_pool.clone();
    let trigger_id: u32 = path.into_inner();
    // Validate
    let _trigger = Trigger::sqlx_by_id_by_user(trigger_id, user.id, &db).await?;
    let mut update: crate::models::UpdateTrigger = data.into_inner().into();
    update.id = Some(trigger_id);
    let trigger = update.sqlx_update(&db).await?;
    Ok(Json(trigger))
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct GetTriggerQueue {
    trigger_id: Option<u32>,
}

#[api_v2_operation]
pub async fn get_trigger_queue_endpoint(
    user: User,
    state: web::Data<AppState>,
) -> Result<Json<Vec<TriggerQueue>>, Error> {
    let db = state.sqlx_pool.clone();
    TriggerQueue::sqlx_by_user(user.id, &db)
        .await
        .map(Json)
        .map_err(|e| e.into())
}

#[api_v2_operation]
pub async fn process_trigger_queue_endpoint(
    user: User,
    state: web::Data<AppState>,
) -> Result<Json<HashMap<u32, Result<TriggerLog, Error>>>, Error> {
    let db = &state.sqlx_pool;
    let queue = TriggerQueue::sqlx_by_user(user.id, &db).await?;
    let trigger_log_map = ultrafinance::sqlx_process_trigger_queue(queue, db).await;
    let mut trigger_log_map_response = HashMap::new();
    for (trigger_queue_id, result) in trigger_log_map {
        trigger_log_map_response
            .insert(trigger_queue_id, result.map_err(|e| -> Error { e.into() }));
    }

    Ok(Json(trigger_log_map_response))
}

#[api_v2_operation]
pub async fn get_trigger_log_endpoint(
    user: User,
    state: web::Data<AppState>,
) -> Result<Json<Vec<TriggerLog>>, Error> {
    let db = state.sqlx_pool.clone();
    let trigger_log = TriggerLog::sqlx_by_user(user.id, &db).await;
    trigger_log.map(Json).map_err(|e| e.into())
}

#[api_v2_operation]
pub async fn delete_trigger_endpoint(
    user: User,
    state: web::Data<AppState>,
    path: web::Path<u32>,
) -> Result<Json<()>, Error> {
    let db = state.sqlx_pool.clone();
    let trigger_id: u32 = path.into_inner();
    let trigger = Trigger::sqlx_by_id_by_user(trigger_id, user.id, &db).await?;
    trigger.sqlx_delete(&db).await?;

    Ok(Json(()))
}
