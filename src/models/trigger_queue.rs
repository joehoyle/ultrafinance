use crate::schema::{self, *};

use cli_table::Table;

use diesel::mysql::Mysql;
use diesel::*;
use diesel::{Associations, Identifiable, MysqlConnection, Queryable};
use paperclip::actix::Apiv2Schema;
use serde::Serialize;

use crate::models::Function;
use crate::models::Trigger;
use crate::models::User;

use anyhow::Result;

use super::{TriggerParams, TriggerLog};
use super::trigger_log::NewTriggerLog;

#[derive(
    Table, Identifiable, Queryable, Associations, ts_rs::TS, Serialize, Apiv2Schema, Selectable, Clone
)]
#[ts(export)]
#[diesel(belongs_to(User))]
#[diesel(belongs_to(Trigger))]
#[diesel(table_name = trigger_queue)]
pub struct TriggerQueue {
    #[table(title = "Queue ID")]
    pub id: i32,
    #[table(skip)]
    pub payload: String,
    #[table(title = "User ID")]
    pub user_id: i32,
    #[table(title = "Trigger ID")]
    pub trigger_id: i32,
    #[table(title = "Date Created")]
    pub created_at: chrono::NaiveDateTime,
    #[table(title = "Updated At")]
    pub updated_at: chrono::NaiveDateTime,
}

type SqlType = diesel::dsl::SqlTypeOf<diesel::dsl::AsSelect<TriggerQueue, Mysql>>;
type BoxedQuery<'a> = crate::schema::trigger_queue::BoxedQuery<'a, Mysql, SqlType>;

impl TriggerQueue {
    pub fn run(&self, con: &mut MysqlConnection) -> anyhow::Result<TriggerLog> {
        let trigger: Trigger = triggers::dsl::triggers.find(self.trigger_id).first(con)?;
        let function: Function = functions::dsl::functions
            .find(trigger.function_id)
            .first(con)?;

        let user: User = users::dsl::users.find(trigger.user_id).first(con)?;

        let mut deno_runtime = crate::deno::FunctionRuntime::new(&function)?;
        let result = deno_runtime.run(&serde_json::to_string::<TriggerParams>(&trigger.params)?, &self.payload);
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
        .create(con)?;

        diesel::delete(
            trigger_queue::dsl::trigger_queue.filter(trigger_queue::dsl::id.eq(self.id)),
        )
        .execute(con)?;
        Ok(log)
    }

    pub fn all() -> BoxedQuery<'static> {
        schema::trigger_queue::table
            .select(Self::as_select())
            .into_boxed()
    }

    pub fn by_user(user: &User) -> BoxedQuery<'static> {
        Self::all().filter(schema::trigger_queue::user_id.eq(user.id))
    }

    pub fn by_id(id: i32, user_id: i32) -> BoxedQuery<'static> {
        Self::all()
            .filter(schema::trigger_queue::id.eq(id))
            .filter(schema::trigger_queue::user_id.eq(user_id))
    }
}

#[derive(Insertable, Default, Debug)]
#[diesel(table_name = trigger_queue)]
pub struct NewTriggerQueue {
    pub payload: String,
    pub user_id: i32,
    pub trigger_id: i32,
}

impl NewTriggerQueue {
    pub fn create(&self, con: &mut MysqlConnection) -> Result<TriggerQueue> {
        use self::trigger_queue::dsl::*;
        match insert_into(trigger_queue).values(self).execute(con) {
            Ok(_) => {
                let trigger_queue_id: i32 = select(schema::last_insert_id()).first(con)?;
                let trigger_queue_id: TriggerQueue =
                    trigger_queue.find(trigger_queue_id).first(con)?;
                Ok(trigger_queue_id)
            }
            Err(e) => Err(e.into()),
        }
    }
}
