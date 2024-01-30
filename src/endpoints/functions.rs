use actix_web::web::{self, block};
use anyhow::Result;
use paperclip::actix::Apiv2Schema;
use paperclip::actix::{api_v2_operation, web::Json};
use serde::{Deserialize, Serialize};

use crate::models::{Function, NewFunction, UpdateFunction, User};
use crate::server::{AppState, Error};

#[derive(Serialize, Apiv2Schema, ts_rs::TS)]
#[ts(rename = "Function", export)]
pub struct FunctionWithParams {
    #[serde(flatten)]
    function: Function,
    #[ts(inline)]
    params: Option<crate::deno::FunctionParams>,
}

#[api_v2_operation]
pub async fn get_functions_endpoint(
    user: User,
    state: web::Data<AppState>,
) -> Result<Json<Vec<FunctionWithParams>>, Error> {
    let db = &state.sqlx_pool;
    let functions = Function::sqlx_by_user(user.id, db).await?;

    let mut functions_with_params = vec![];
    for function in functions {
        let params = function.get_params().await.ok();
        functions_with_params.push(FunctionWithParams {
            function,
            params,
        })
    }

    Ok(Json(functions_with_params))
}

#[api_v2_operation]
pub async fn get_function_endpoint(
    user: User,
    state: web::Data<AppState>,
    path: web::Path<u32>,
) -> Result<Json<FunctionWithParams>, Error> {
    let user = user.clone();
    let db = &state.sqlx_pool;
    let function_id: u32 = path.into_inner();
    let function = Function::sqlx_by_id_by_user(function_id, user.id, db).await?;
    Ok(Json(FunctionWithParams {
        params: function.get_params().await.ok(),
        function: function,
    }))
}

#[derive(ts_rs::TS, Deserialize, Apiv2Schema)]
#[ts(export)]
pub struct CreateFunction {
    pub name: String,
    pub source: String,
}

#[api_v2_operation]
pub async fn create_function_endpoint(
    user: User,
    state: web::Data<AppState>,
    data: web::Json<CreateFunction>,
) -> Result<Json<Function>, Error> {
    let db = &state.sqlx_pool;
    let function = NewFunction {
        name: data.name.clone(),
        function_type: "user".into(),
        source: data.source.clone(),
        user_id: user.id,
    }
    .sqlx_create(db)
    .await?;
    Ok(Json(function))
}

#[api_v2_operation]
pub async fn update_function_endpoint(
    user: User,
    state: web::Data<AppState>,
    data: web::Json<UpdateFunction>,
    path: web::Path<u32>,
) -> Result<Json<Function>, Error> {
    let db = &state.sqlx_pool;
    let function_id: u32 = path.into_inner();
    let mut update_function = data.into_inner();
    update_function.id = Some(function_id);
    // Validate function.
    Function::sqlx_by_id_by_user(function_id, user.id, db).await?;
    let function = update_function.sqlx_update(db).await?;
    Ok(Json(function))
}

#[derive(Deserialize, Apiv2Schema, ts_rs::TS)]
#[ts(export)]
pub struct TestFunction {
    pub params: String,
    pub payload: String,
}

#[api_v2_operation]
pub async fn test_function_endpoint(
    user: User,
    state: web::Data<AppState>,
    data: web::Json<TestFunction>,
    path: web::Path<u32>,
) -> Result<Json<String>, Error> {
    let db = &state.sqlx_pool;
    let function_id: u32 = path.into_inner();
    let function = Function::sqlx_by_id_by_user(function_id, user.id, db).await?;
    let test_data = data.into_inner();
    let mut deno_runtime = crate::deno::FunctionRuntime::new(&function).await?;
    let function = deno_runtime
        .run(&test_data.params, &test_data.payload)
        .await?;

    Ok(Json(function))
}

#[api_v2_operation]
pub async fn delete_function_endpoint(
    user: User,
    state: web::Data<AppState>,
    path: web::Path<u32>,
) -> Result<Json<()>, Error> {
    let db = &state.sqlx_pool;
    let function_id: u32 = path.into_inner();
    let function = Function::sqlx_by_id_by_user(function_id, user.id, db).await?;
    function.sqlx_delete(db).await?;
    Ok(Json(()))
}
