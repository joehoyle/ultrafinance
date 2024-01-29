use cli_table::Table;

use paperclip::actix::Apiv2Schema;
use serde::Serialize;

use anyhow::Result;

#[derive(Table, Debug, ts_rs::TS, Serialize, Apiv2Schema)]
#[ts(export)]
pub struct TriggerLog {
    #[table(title = "Log ID")]
    pub id: u32,
    #[table(title = "Payload")]
    pub payload: String,
    #[table(title = "Status")]
    pub status: String,
    #[table(title = "User ID")]
    pub user_id: u32,
    #[table(title = "Trigger ID")]
    pub trigger_id: u32,
    #[table(title = "Date Created")]
    pub created_at: chrono::NaiveDateTime,
    #[table(title = "Updated At")]
    pub updated_at: chrono::NaiveDateTime,
}

impl TriggerLog {
    pub async fn sqlx_all(db: &sqlx::MySqlPool) -> Result<Vec<Self>, anyhow::Error> {
        sqlx::query_as!(Self, "SELECT * FROM trigger_log")
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_id(id: u32, db: &sqlx::MySqlPool) -> Result<Self, anyhow::Error> {
        sqlx::query_as!(Self, "SELECT * FROM trigger_log WHERE id = ?", id)
            .fetch_one(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_user(
        user_id: u32,
        db: &sqlx::MySqlPool,
    ) -> Result<Vec<Self>, anyhow::Error> {
        sqlx::query_as!(Self, "SELECT * FROM trigger_log WHERE user_id = ?", user_id)
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }
}

#[derive(Default, Debug, sqlx::FromRow)]
pub struct NewTriggerLog {
    pub payload: String,
    pub status: String,
    pub user_id: u32,
    pub trigger_id: u32,
}

impl NewTriggerLog {
    pub async fn sqlx_create(self, db: &sqlx::MySqlPool) -> Result<TriggerLog, anyhow::Error> {
        let result = sqlx::query!(
            "INSERT INTO trigger_log (payload, status, user_id, trigger_id) VALUES (?, ?, ?, ?)",
            self.payload,
            self.status,
            self.user_id,
            self.trigger_id
        )
        .execute(db)
        .await?;
        TriggerLog::sqlx_by_id(result.last_insert_id() as u32, db).await
    }
}
