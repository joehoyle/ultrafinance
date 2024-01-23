use crate::nordigen::Requisition;
use crate::schema::*;

use cli_table::Table;

use diesel::*;
use diesel::{Associations, Identifiable, MysqlConnection, Queryable};

use serde::Serialize;

use crate::models::User;

#[derive(Table, Identifiable, Queryable, Associations, Debug, Serialize, ts_rs::TS)]
#[ts(export)]
#[diesel(belongs_to(User))]
#[derive(sqlx::FromRow)]
pub struct NordigenRequisition {
    #[table(title = "Requisition ID")]
    pub id: u32,
    #[table(title = "Nordigen ID")]
    #[serde(skip_serializing)]
    pub nordigen_id: String,
    #[table(title = "Status")]
    pub status: String,
    #[table(title = "User ID")]
    #[serde(skip_serializing)]
    pub user_id: u32,
    #[table(title = "Date Created")]
    pub created_at: chrono::NaiveDateTime,
    #[table(title = "Updated At")]
    pub updated_at: chrono::NaiveDateTime,
}

impl NordigenRequisition {
    pub async fn sqlx_all(db: &sqlx::MySqlPool) -> Result<Vec<Self>, anyhow::Error> {
        sqlx::query_as!(Self, "SELECT * FROM nordigen_requisitions")
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_id(id: u32, db: &sqlx::MySqlPool) -> Result<Self, anyhow::Error> {
        sqlx::query_as!(Self, "SELECT * FROM nordigen_requisitions WHERE id = ?", id)
            .fetch_one(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }
}

pub fn create_nordigen_requisition(
    requisition: &Requisition,
    user: &User,
    con: &mut MysqlConnection,
) -> Result<(), diesel::result::Error> {
    use self::nordigen_requisitions::dsl::*;
    match insert_into(nordigen_requisitions)
        .values((
            nordigen_id.eq(requisition.id.clone()),
            status.eq(requisition.status.clone()),
            user_id.eq(user.id),
        ))
        .execute(con)
    {
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}

pub async fn sqlx_create_nordigen_requisition(
    requisition: &Requisition,
    user: &User,
    con: &sqlx::MySqlPool,
) -> Result<(), anyhow::Error> {
    sqlx::query!(
        "INSERT INTO nordigen_requisitions (nordigen_id, status, user_id) VALUES (?, ?, ?)",
        requisition.id.clone(),
        requisition.status.clone(),
        user.id,
    )
    .execute(con)
    .await
    .map_err(|e| anyhow::anyhow!(e))
    .map(|_| ())
}
