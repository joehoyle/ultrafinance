use cli_table::Table;
use paperclip::actix::Apiv2Schema;
use serde::{Deserialize, Serialize};

use std::collections::HashMap;

use anyhow::Result;

use crate::models::Transaction;

#[derive(Deserialize, Serialize, Debug, ts_rs::TS, Apiv2Schema)]
#[ts(export)]
pub enum TriggerFilterPredicate {
    Account(Vec<u32>),
}

#[derive(Deserialize, Serialize, Debug, ts_rs::TS, Apiv2Schema, Default)]
#[ts(export)]
pub struct TriggerFilter(pub Vec<TriggerFilterPredicate>);

#[derive(Deserialize, Serialize, Debug, ts_rs::TS, Apiv2Schema, Default)]
#[ts(export)]
pub struct TriggerParams(pub HashMap<String, String>);

impl From<String> for TriggerParams {
    fn from(s: String) -> Self {
        serde_json::from_str(&s).unwrap()
    }
}

impl TriggerFilter {
    pub fn matches(&self, transaction: &Transaction) -> bool {
        for filter in &self.0 {
            let matches = match filter {
                TriggerFilterPredicate::Account(account_ids) => {
                    account_ids.contains(&transaction.account_id)
                }
            };
            if !matches {
                return false;
            }
        }
        true
    }
}

impl From<String> for TriggerFilter {
    fn from(s: String) -> Self {
        serde_json::from_str(&s).unwrap()
    }
}

#[derive(Table, Debug, Serialize, ts_rs::TS, Apiv2Schema)]
#[ts(export)]
#[derive(sqlx::FromRow)]
pub struct Trigger {
    #[table(title = "Trigger ID")]
    pub id: u32,
    #[table(title = "Event")]
    pub event: String,
    #[table(title = "Name")]
    pub name: String,
    #[table(skip)]
    pub filter: TriggerFilter,
    #[table(skip)]
    pub params: TriggerParams,
    #[table(title = "User ID")]
    #[serde(skip_serializing)]
    pub user_id: u32,
    #[table(title = "Function ID")]
    pub function_id: u32,
    #[table(title = "Date Created")]
    pub created_at: chrono::NaiveDateTime,
    #[table(title = "Updated At")]
    pub updated_at: chrono::NaiveDateTime,
}

impl Trigger {
    pub async fn sqlx_all(db: &sqlx::MySqlPool) -> Result<Vec<Self>, anyhow::Error> {
        sqlx::query_as!(Self, "SELECT * FROM triggers")
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_id(id: u32, db: &sqlx::MySqlPool) -> Result<Self, anyhow::Error> {
        sqlx::query_as!(Self, "SELECT * FROM triggers WHERE id = ?", id)
            .fetch_one(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_user(
        user_id: u32,
        db: &sqlx::MySqlPool,
    ) -> Result<Vec<Self>, anyhow::Error> {
        sqlx::query_as!(Self, "SELECT * FROM triggers WHERE user_id = ?", user_id)
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_id_by_user(
        id: u32,
        user_id: u32,
        db: &sqlx::MySqlPool,
    ) -> Result<Self, anyhow::Error> {
        sqlx::query_as!(
            Self,
            "SELECT * FROM triggers WHERE id = ? AND user_id = ?",
            id,
            user_id
        )
        .fetch_one(db)
        .await
        .map_err(|e| anyhow::anyhow!(e))
    }
    pub async fn sqlx_for_user_for_event(
        user_id: u32,
        event: &str,
        db: &sqlx::MySqlPool,
    ) -> Result<Vec<Self>, anyhow::Error> {
        sqlx::query_as!(
            Self,
            "SELECT * FROM triggers WHERE user_id = ? AND event = ?",
            user_id,
            event
        )
        .fetch_all(db)
        .await
        .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_delete(self, db: &sqlx::MySqlPool) -> Result<(), anyhow::Error> {
        sqlx::query("DELETE FROM triggers WHERE id = ?")
            .bind(self.id)
            .execute(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(())
    }
}

#[derive(Default, Debug)]
pub struct NewTrigger {
    pub event: String,
    pub name: String,
    pub filter: TriggerFilter,
    pub params: String,
    pub user_id: u32,
    pub function_id: u32,
}

impl NewTrigger {
    pub async fn sqlx_create(self, db: &sqlx::MySqlPool) -> Result<Trigger, anyhow::Error> {
        let result = sqlx::query!(
            "INSERT INTO triggers (event, name, filter, params, user_id, function_id) VALUES (?, ?, ?, ?, ?, ?)",
            self.event,
            self.name,
            serde_json::to_string(&self.filter).unwrap(),
            serde_json::to_string(&self.params).unwrap(),
            self.user_id, self.function_id)
            .execute(db)
            .await?;
        Trigger::sqlx_by_id(result.last_insert_id() as u32, db).await
    }
}

#[derive(Deserialize, Apiv2Schema, ts_rs::TS)]
#[ts(export)]
pub struct UpdateTrigger {
    pub id: Option<u32>,
    pub name: Option<String>,
    pub params: Option<String>,
    pub event: Option<String>,
    pub filter: Option<TriggerFilter>,
    pub function_id: Option<u32>,
}

impl UpdateTrigger {
    pub async fn sqlx_update(self, db: &sqlx::MySqlPool) -> Result<Trigger, anyhow::Error> {
        let _ = sqlx::query("UPDATE triggers SET name = ?, params = ?, event = ?, filter = ?, function_id = ?, updated_at = ? WHERE id = ?")
            .bind(&self.name)
            .bind(&self.params)
            .bind(&self.event)
            .bind(serde_json::to_string(&self.filter).unwrap())
            .bind(&self.function_id)
            .bind(&self.id)
            .execute(db)
            .await.map_err(|e| anyhow::anyhow!(e))?;
        Trigger::sqlx_by_id(self.id.unwrap(), db).await
    }
}
