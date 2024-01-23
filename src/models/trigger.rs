use crate::schema::*;

use cli_table::Table;
use diesel::deserialize::FromSql;
use diesel::mysql::{Mysql, MysqlValue};
use diesel::serialize::{IsNull, Output, ToSql};
use diesel::sql_types::Text;
use diesel::*;
use diesel::{Associations, Identifiable, MysqlConnection, Queryable};
use paperclip::actix::Apiv2Schema;
use serde::{Deserialize, Serialize};

use std::collections::HashMap;
use std::error::Error;

use anyhow::Result;

use crate::models::Function;
use crate::models::Transaction;
use crate::models::User;
use crate::schema;

type SqlType = diesel::dsl::SqlTypeOf<diesel::dsl::AsSelect<Trigger, Mysql>>;
type BoxedQuery<'a> = crate::schema::triggers::BoxedQuery<'a, Mysql, SqlType>;

#[derive(Deserialize, Serialize, Debug, ts_rs::TS, Apiv2Schema)]
#[ts(export)]
pub enum TriggerFilterPredicate {
    Account(Vec<u32>),
}

#[derive(
    Deserialize, Serialize, Debug, FromSqlRow, ts_rs::TS, Apiv2Schema, AsExpression, Default,
)]
#[diesel(sql_type = Text)]
#[ts(export)]
pub struct TriggerFilter(pub Vec<TriggerFilterPredicate>);

#[derive(
    Deserialize, Serialize, Debug, FromSqlRow, ts_rs::TS, Apiv2Schema, AsExpression, Default,
)]
#[diesel(sql_type = Text)]
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

impl ToSql<Text, Mysql> for TriggerFilter {
    fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Mysql>) -> serialize::Result {
        serde_json::to_writer(out, self)
            .map(|_| IsNull::No)
            .map_err(|e| Box::new(e) as Box<dyn Error + Send + Sync>)
    }
}

impl FromSql<Text, Mysql> for TriggerFilter {
    fn from_sql(bytes: MysqlValue<'_>) -> Result<Self, Box<dyn Error + Send + Sync>> {
        serde_json::from_slice(bytes.as_bytes())
            .map_err(|e| Box::new(e) as Box<dyn Error + Send + Sync>)
    }
}

impl ToSql<Text, Mysql> for TriggerParams {
    fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Mysql>) -> serialize::Result {
        serde_json::to_writer(out, self)
            .map(|_| IsNull::No)
            .map_err(|e| Box::new(e) as Box<dyn Error + Send + Sync>)
    }
}

impl FromSql<Text, Mysql> for TriggerParams {
    fn from_sql(bytes: MysqlValue<'_>) -> Result<Self, Box<dyn Error + Send + Sync>> {
        serde_json::from_slice(bytes.as_bytes())
            .map_err(|e| Box::new(e) as Box<dyn Error + Send + Sync>)
    }
}

#[derive(
    Table,
    Identifiable,
    Queryable,
    Associations,
    Debug,
    Serialize,
    ts_rs::TS,
    Apiv2Schema,
    Selectable,
)]
#[ts(export)]
#[diesel(belongs_to(User))]
#[diesel(belongs_to(Function))]
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
    pub fn all() -> BoxedQuery<'static> {
        schema::triggers::table
            .select(Self::as_select())
            .into_boxed()
    }

    pub fn by_user(user_id: u32) -> BoxedQuery<'static> {
        Self::all().filter(schema::triggers::user_id.eq(user_id))
    }

    pub fn by_id(id: u32, user_id: u32) -> BoxedQuery<'static> {
        Self::all()
            .filter(schema::triggers::id.eq(id))
            .filter(schema::triggers::user_id.eq(user_id))
    }

    pub fn delete(self, con: &mut MysqlConnection) -> Result<()> {
        diesel::delete(&self).execute(con)?;
        Ok(())
    }

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

    pub async fn sqlx_for_user_for_event(user_id: u32, event: &str, db: &sqlx::MySqlPool) -> Result<Vec<Self>, anyhow::Error> {
        sqlx::query_as!(Self, "SELECT * FROM triggers WHERE user_id = ? AND event = ?", user_id, event)
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

}

#[derive(Insertable, Default, Debug)]
#[diesel(table_name = triggers)]
pub struct NewTrigger {
    pub event: String,
    pub name: String,
    pub filter: TriggerFilter,
    pub params: String,
    pub user_id: u32,
    pub function_id: u32,
}

impl NewTrigger {
    pub fn create(&self, con: &mut MysqlConnection) -> Result<Trigger> {
        use self::triggers::dsl::*;
        match insert_into(triggers).values(self).execute(con) {
            Ok(_) => {
                let trigger_id: u32 = select(schema::last_insert_id()).first(con)?;
                let trigger: Trigger = triggers.find(trigger_id).first(con)?;
                Ok(trigger)
            }
            Err(e) => Err(e.into()),
        }
    }

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

#[derive(AsChangeset, Deserialize, Apiv2Schema, ts_rs::TS)]
#[diesel(table_name = triggers)]
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
    pub fn update(self, con: &mut MysqlConnection) -> Result<()> {
        use self::triggers::dsl::*;
        diesel::update(triggers)
            .filter(id.eq(self.id.ok_or(anyhow::anyhow!("No id found"))?))
            .set((&self, updated_at.eq(chrono::offset::Utc::now().naive_utc())))
            .execute(con)
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(())
    }
}
