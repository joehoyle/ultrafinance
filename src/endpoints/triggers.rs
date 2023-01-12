use std::collections::HashMap;

use actix_web::web::{self, block};
use paperclip::actix::Apiv2Schema;
use paperclip::actix::{api_v2_operation, web::Json};
use serde::Deserialize;

use crate::models::{Function, NewTrigger, Trigger, TriggerFilter, TriggerLog, TriggerQueue, User};
use crate::{AppState, Error, ultrafinance};

use diesel::*;

#[derive(Apiv2Schema)]
#[allow(dead_code)]
pub struct GetTriggersQuery {
    function_id: Option<i32>,
}

#[api_v2_operation]
pub async fn get_triggers_endpoint(
    user: User,
    state: web::Data<AppState>,
) -> Result<Json<Vec<Trigger>>, Error> {
    let db = state.db.clone();
    let triggers = block(move || -> Result<Vec<Trigger>, Error> {
        use diesel::*;
        let mut con = db.get()?;
        Trigger::by_user(user.id)
            .load(&mut con)
            .map_err(|e| e.into())
    })
    .await
    .unwrap();

    triggers.map(Json)
}

#[api_v2_operation]
pub async fn get_trigger_endpoint(
    user: User,
    state: web::Data<AppState>,
    path: web::Path<i32>,
) -> Result<Json<Trigger>, Error> {
    let db = state.db.clone();
    let trigger_id: i32 = path.into_inner();
    let trigger = block(move || -> Result<Trigger, Error> {
        use diesel::*;
        let mut con = db.get()?;
        Trigger::by_id(trigger_id, user.id)
            .first(&mut con)
            .map_err(|e| e.into())
    })
    .await
    .unwrap();

    trigger.map(Json)
}

#[derive(Deserialize, Apiv2Schema, ts_rs::TS)]
#[ts(export)]
pub struct CreateTrigger {
    function_id: i32,
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
    let db = state.db.clone();
    let trigger = block(move || -> Result<Trigger, Error> {
        let mut con = db.get()?;
        // Validate the function id
        Function::by_id(data.function_id, user.id).first(&mut con)?;
        NewTrigger {
            event: data.event.clone(),
            name: data.name.clone(),
            filter: TriggerFilter(vec![]), // todo
            params: serde_json::to_string(&data.params)?,
            user_id: user.id,
            function_id: data.function_id,
        }
        .create(&mut con)
        .map_err(|e| e.into())
    })
    .await
    .unwrap();
    trigger.map(Json)
}

#[derive(Deserialize, Apiv2Schema, ts_rs::TS)]
#[ts(export)]
pub struct UpdateTrigger {
    function_id: Option<i32>,
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
    path: web::Path<i32>,
) -> Result<Json<Trigger>, Error> {
    let db = state.db.clone();
    let trigger_id: i32 = path.into_inner();
    let trigger = block(move || -> Result<Trigger, Error> {
        let mut con = db.get()?;
        let data = data.into_inner();
        Trigger::by_id(trigger_id, user.id).first(&mut con)?;
        let mut update: crate::models::UpdateTrigger = data.into();
        update.id = Some(trigger_id);
        update.update(&mut con)?;
        Trigger::by_id(trigger_id, user.id)
            .first(&mut con)
            .map_err(|e| e.into())
    })
    .await
    .unwrap();

    trigger.map(Json)
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct GetTriggerQueue {
    trigger_id: Option<i32>,
}

#[api_v2_operation]
pub async fn get_trigger_queue_endpoint(
    user: User,
    state: web::Data<AppState>,
) -> Result<Json<Vec<TriggerQueue>>, Error> {
    let db = state.db.clone();
    let trigger_queue = block(move || -> Result<Vec<TriggerQueue>, Error> {
        use diesel::*;
        let mut con = db.get()?;
        TriggerQueue::by_user(&user)
            .load(&mut con)
            .map_err(|e| e.into())
    })
    .await
    .unwrap();

    trigger_queue.map(Json)
}

#[api_v2_operation]
pub async fn process_trigger_queue_endpoint(
    user: User,
    state: web::Data<AppState>,
) -> Result<Json<HashMap<i32, Result<TriggerLog, Error>>>, Error> {
    let db = state.db.clone();

    let logs = block(move || -> Result<HashMap<i32, Result<TriggerLog, Error>>, Error> {
        use diesel::*;
        let mut con = db.get()?;
        let queue = TriggerQueue::by_user(&user).load(&mut con).map_err(|e| -> Error { e.into() })?;
        let trigger_log_map = ultrafinance::process_trigger_queue(&queue, db);
        let mut trigger_log_map_response = HashMap::new();
        for (trigger_queue_id, result) in trigger_log_map {
            trigger_log_map_response.insert(trigger_queue_id, result.map_err(|e| -> Error { e.into() }));
        }

        Ok(trigger_log_map_response)
    })
    .await
    .unwrap();

    logs.map(Json)
}

#[api_v2_operation]
pub async fn get_trigger_log_endpoint(
    user: User,
    state: web::Data<AppState>,
) -> Result<Json<Vec<TriggerLog>>, Error> {
    let db = state.db.clone();
    let trigger_log = block(move || -> Result<Vec<TriggerLog>, Error> {
        use diesel::*;
        let mut con = db.get()?;
        TriggerLog::by_user(&user)
            .load(&mut con)
            .map_err(|e| e.into())
    })
    .await
    .unwrap();

    trigger_log.map(Json)
}


#[api_v2_operation]
pub async fn delete_trigger_endpoint(
    user: User,
    state: web::Data<AppState>,
    path: web::Path<i32>,
) -> Result<Json<()>, Error> {
    let db = state.db.clone();
    let trigger_id: i32 = path.into_inner();
    let trigger = block(move || -> Result<(), Error> {
        use diesel::*;
        let mut con = db.get()?;
        let trigger = Trigger::by_id(trigger_id, user.id).first(&mut con)?;
        trigger.delete(&mut con).map_err(|e| e.into())
    })
    .await
    .unwrap();

    trigger.map(Json)
}

