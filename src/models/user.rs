use cli_table::Table;

use paperclip::actix::Apiv2Schema;
use serde::{Deserialize, Serialize};

use anyhow::Result;

use crate::ultrafinance::{hash_api_key, hash_password};

#[derive(Table, Debug, Serialize, ts_rs::TS, Apiv2Schema, Clone, sqlx::FromRow)]
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

impl User {
    pub async fn sqlx_all(db: &sqlx::MySqlPool) -> Result<Vec<Self>, anyhow::Error> {
        sqlx::QueryBuilder::new("SELECT * FROM users")
            .build_query_as::<Self>()
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_id(id: u32, db: &sqlx::MySqlPool) -> Result<Self, anyhow::Error> {
        sqlx::query_as!(Self, "SELECT * FROM users WHERE id = ?", id)
            .fetch_one(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_email(email: &str, db: &sqlx::MySqlPool) -> Result<Self, anyhow::Error> {
        sqlx::query_as!(Self, "SELECT * FROM users WHERE email = ?", email)
            .fetch_one(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }
}

#[derive(Default, Debug, Apiv2Schema, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NewUser {
    pub name: String,
    pub email: String,
    pub password: String,
}

impl NewUser {
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

#[derive(Deserialize, Apiv2Schema, ts_rs::TS, Debug)]
#[ts(export)]
pub struct UpdateUser {
    pub id: Option<u32>,
    pub name: Option<String>,
    pub email: Option<String>,
    pub password: Option<String>,
}

impl UpdateUser {
    pub async fn sqlx_update(self, db: &sqlx::MySqlPool) -> Result<User, anyhow::Error> {
        let _ = sqlx::query("UPDATE users SET name = ?, email = ?, password = ? WHERE id = ?")
            .bind(&self.name)
            .bind(&self.email)
            .bind(&self.password)
            .bind(&self.id)
            .execute(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        User::sqlx_by_id(self.id.unwrap(), db).await
    }
}

pub async fn create_api_key(user: &User, con: &sqlx::MySqlPool) -> Result<String, anyhow::Error> {
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
