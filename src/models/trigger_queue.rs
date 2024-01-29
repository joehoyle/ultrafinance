use cli_table::Table;

use paperclip::actix::Apiv2Schema;
use serde::Serialize;

use crate::models::Function;
use crate::models::Trigger;
use crate::models::User;

use anyhow::Result;

use super::trigger_log::NewTriggerLog;
use super::{TriggerLog, TriggerParams};

#[derive(Table, ts_rs::TS, Serialize, Apiv2Schema, Clone)]
#[ts(export)]
#[derive(sqlx::FromRow)]
pub struct TriggerQueue {
    #[table(title = "Queue ID")]
    pub id: u32,
    #[table(skip)]
    pub payload: String,
    #[table(title = "User ID")]
    pub user_id: u32,
    #[table(title = "Trigger ID")]
    pub trigger_id: u32,
    #[table(title = "Date Created")]
    pub created_at: chrono::NaiveDateTime,
    #[table(title = "Updated At")]
    pub updated_at: chrono::NaiveDateTime,
}

impl TriggerQueue {
    pub async fn sqlx_run(self, db: &sqlx::MySqlPool) -> anyhow::Result<TriggerLog> {
        let trigger = Trigger::sqlx_by_id(self.trigger_id, db).await?;
        let function = Function::sqlx_by_id(trigger.function_id, db).await?;
        let user = User::sqlx_by_id(self.user_id, db).await?;

        let mut deno_runtime = crate::deno::FunctionRuntime::new(&function)?;
        let result = deno_runtime.run(
            &serde_json::to_string::<TriggerParams>(&trigger.params)?,
            &self.payload,
        );
        let payload = match &result {
            Ok(p) => (p.clone(), "completed"),
            Err(e) => (e.to_string(), "failed"),
        };

        let log = NewTriggerLog {
            payload: payload.0,
            status: payload.1.to_owned(),
            user_id: user.id,
            trigger_id: trigger.id,
        }
        .sqlx_create(db)
        .await?;

        self.sqlx_delete(db).await?;
        Ok(log)
    }

    pub async fn sqlx_all(db: &sqlx::MySqlPool) -> Result<Vec<Self>, anyhow::Error> {
        sqlx::query_as!(Self, "SELECT * FROM trigger_queue")
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_id(id: u32, db: &sqlx::MySqlPool) -> Result<Self, anyhow::Error> {
        sqlx::query_as!(Self, "SELECT * FROM trigger_queue WHERE id = ?", id)
            .fetch_one(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_user(
        user_id: u32,
        db: &sqlx::MySqlPool,
    ) -> Result<Vec<Self>, anyhow::Error> {
        sqlx::query_as!(
            Self,
            "SELECT * FROM trigger_queue WHERE user_id = ?",
            user_id
        )
        .fetch_all(db)
        .await
        .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_delete(self, db: &sqlx::MySqlPool) -> Result<(), anyhow::Error> {
        sqlx::query!("DELETE FROM trigger_queue WHERE id = ?", self.id)
            .execute(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(())
    }
}

#[derive(Default, Debug)]
pub struct NewTriggerQueue {
    pub payload: String,
    pub user_id: u32,
    pub trigger_id: u32,
}

impl NewTriggerQueue {
    pub async fn sqlx_create(self, db: &sqlx::MySqlPool) -> Result<TriggerQueue, anyhow::Error> {
        let result = sqlx::query!(
            "INSERT INTO trigger_queue (payload, user_id, trigger_id) VALUES (?, ?, ?)",
            self.payload,
            self.user_id,
            self.trigger_id
        )
        .execute(db)
        .await?;
        TriggerQueue::sqlx_by_id(result.last_insert_id() as u32, db).await
    }
}
