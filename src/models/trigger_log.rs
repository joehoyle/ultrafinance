use crate::schema::{self, *};

use cli_table::Table;

use diesel::mysql::Mysql;
use diesel::*;
use diesel::{Associations, Identifiable, MysqlConnection, Queryable};
use paperclip::actix::Apiv2Schema;
use serde::Serialize;

use anyhow::Result;

use crate::models::Trigger;
use crate::models::User;

#[derive(
    Table,
    Identifiable,
    Queryable,
    Associations,
    Debug,
    ts_rs::TS,
    Serialize,
    Apiv2Schema,
    Selectable,
)]
#[ts(export)]
#[diesel(belongs_to(User))]
#[diesel(belongs_to(Trigger))]
#[diesel(table_name = trigger_log)]
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

type SqlType = diesel::dsl::SqlTypeOf<diesel::dsl::AsSelect<TriggerLog, Mysql>>;
type BoxedQuery<'a> = crate::schema::trigger_log::BoxedQuery<'a, Mysql, SqlType>;

impl TriggerLog {
    pub fn all() -> BoxedQuery<'static> {
        schema::trigger_log::table
            .select(Self::as_select())
            .into_boxed()
    }

    pub fn by_user(user: &User) -> BoxedQuery<'static> {
        Self::all().filter(schema::trigger_log::user_id.eq(user.id))
    }

    pub fn by_id(id: u32, user_id: u32) -> BoxedQuery<'static> {
        Self::all()
            .filter(schema::trigger_log::id.eq(id))
            .filter(schema::trigger_log::user_id.eq(user_id))
    }

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
}

#[derive(Insertable, Default, Debug)]
#[diesel(table_name = trigger_log)]
#[derive(sqlx::FromRow)]
pub struct NewTriggerLog {
    pub payload: String,
    pub status: String,
    pub user_id: u32,
    pub trigger_id: u32,
}

impl NewTriggerLog {
    pub fn create(&self, con: &mut MysqlConnection) -> Result<TriggerLog> {
        use self::trigger_log::dsl::*;
        match insert_into(trigger_log).values(self).execute(con) {
            Ok(_) => {
                let trigger_log_id_id: u32 = select(schema::last_insert_id()).first(con)?;
                let trigger_log_id: TriggerLog = trigger_log.find(trigger_log_id_id).first(con)?;
                Ok(trigger_log_id)
            }
            Err(e) => Err(e.into()),
        }
    }

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
