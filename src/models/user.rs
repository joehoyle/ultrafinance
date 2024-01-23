use crate::schema::{self, *};

use cli_table::Table;

use diesel::mysql::Mysql;
use diesel::*;
use diesel::{Identifiable, MysqlConnection, Queryable};
use paperclip::actix::Apiv2Schema;
use serde::{Deserialize, Serialize};

use anyhow::Result;
use sqlx::query::QueryAs;
use sqlx::QueryBuilder;

use crate::ultrafinance::{hash_api_key, hash_password};

#[derive(
    Table,
    Identifiable,
    Queryable,
    Debug,
    Serialize,
    ts_rs::TS,
    Apiv2Schema,
    Clone,
    Selectable,
    sqlx::FromRow,
)]
#[ts(export)]
pub struct User {
    #[table(title = "User ID")]
    pub id: u32,
    #[table(title = "Name")]
    pub name: String,
    #[table(title = "Email")]
    pub email: String,
    #[serde(skip_serializing)]
    pub(crate) password: String,
    #[table(title = "Date Created")]
    pub created_at: chrono::NaiveDateTime,
    #[table(title = "Updated At")]
    pub updated_at: chrono::NaiveDateTime,
}

type SqlType = diesel::dsl::SqlTypeOf<diesel::dsl::AsSelect<User, Mysql>>;
type BoxedQuery<'a> = crate::schema::users::BoxedQuery<'a, Mysql, SqlType>;

impl User {
    pub fn all() -> BoxedQuery<'static> {
        schema::users::table.select(Self::as_select()).into_boxed()
    }

    pub fn by_id(id: u32) -> BoxedQuery<'static> {
        Self::all().filter(schema::users::id.eq(id))
    }

    pub async fn sqlx_all(db: &sqlx::MySqlPool) -> Result<Vec<Self>, anyhow::Error> {
        sqlx::QueryBuilder::new("SELECT * FROM users")
            .build_query_as::<Self>()
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_id(id: u32, db: &sqlx::MySqlPool) -> Result<Self, anyhow::Error> {
        sqlx::QueryBuilder::new("SELECT * FROM users WHERE id = ?")
            .push_bind(id)
            .build_query_as::<Self>()
            .fetch_one(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }
}

#[derive(Insertable, Default, Debug, Apiv2Schema, Deserialize, ts_rs::TS)]
#[diesel(table_name = users)]
#[ts(export)]
pub struct NewUser {
    pub name: String,
    pub email: String,
    pub password: String,
}

impl NewUser {
    pub fn create(mut self, con: &mut MysqlConnection) -> Result<User> {
        use self::users::dsl::*;
        self.password = hash_password(&self.password)?;
        match insert_into(users).values(self).execute(con) {
            Ok(_) => {
                let user_id: u32 = select(schema::last_insert_id()).first(con)?;
                let user: User = users.find(user_id).first(con)?;
                Ok(user)
            }
            Err(e) => Err(e.into()),
        }
    }

    pub async fn sqlx_create(self, db: &sqlx::MySqlPool) -> Result<User, anyhow::Error> {
        let result = sqlx::query("INSERT INTO users (name, email, password) VALUES (?, ?, ?)")
            .bind(self.name)
            .bind(self.email)
            .bind(hash_password(&self.password)?)
            .execute(db)
            .await?;
        User::sqlx_by_id(result.last_insert_id() as u32, db).await
    }
}

#[derive(AsChangeset, Deserialize, Apiv2Schema, ts_rs::TS, Debug, Identifiable)]
#[diesel(table_name = users)]
#[ts(export)]
pub struct UpdateUser {
    pub id: Option<u32>,
    pub name: Option<String>,
    pub email: Option<String>,
    pub password: Option<String>,
}

impl UpdateUser {
    pub fn update(mut self, con: &mut MysqlConnection) -> Result<()> {
        match &self.password {
            Some(p) => self.password = Some(hash_password(p.as_str())?),
            None => (),
        }

        use self::users::dsl::*;
        let update_statement = diesel::update(users)
            .filter(id.eq(self.id.ok_or(anyhow::anyhow!("No id found"))?))
            .set((&self, updated_at.eq(chrono::offset::Utc::now().naive_utc())));
        dbg!(diesel::debug_query::<Mysql, _>(&update_statement));
        update_statement
            .execute(con)
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(())
    }
}

pub async fn create_api_key(
    user: &User,
    con: &sqlx::MySqlPool,
) -> Result<String, anyhow::Error> {
    use uuid::Uuid;
    let raw_api_key = Uuid::new_v4().to_string();
    let ph = hash_api_key(raw_api_key.as_str());

    sqlx::query("INSERT INTO user_api_keys (user_id, api_key) VALUES (?, ?)")
        .bind(user.id)
        .bind(ph)
        .execute(con)
        .await?;

    Ok(raw_api_key)
}
