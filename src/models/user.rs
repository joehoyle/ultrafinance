use cli_table::Table;
use serde::{Deserialize, Serialize};
use anyhow::Result;

#[derive(Table, Debug, Serialize, Clone, sqlx::FromRow)]
pub struct User {
    #[table(title = "User ID")]
    pub id: u32,
    #[table(title = "Name")]
    pub name: String,
    #[table(title = "Email")]
    pub email: String,
    #[serde(skip_serializing)]
    #[table(skip)]
    #[allow(dead_code)]
    pub(crate) password: String,
    // #[table(title = "Currency")]
    // pub primary_currency: Currency,
    #[table(title = "Date Created")]
    pub created_at: chrono::NaiveDateTime,
    #[table(title = "Updated At")]
    pub updated_at: chrono::NaiveDateTime,
}

impl User {
    pub async fn sqlx_all(db: &sqlx::MySqlPool) -> Result<Vec<Self>, anyhow::Error> {
        sqlx::query_as!(Self, "SELECT * FROM users")
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

#[derive(Default, Debug, Deserialize)]

pub struct NewUser {
    pub name: String,
    pub email: String,
}

impl NewUser {
    pub async fn sqlx_create(self, db: &sqlx::MySqlPool) -> Result<User, anyhow::Error> {
        let result = sqlx::query("INSERT INTO users (name, email, password) VALUES (?, ?, ?)")
            .bind(self.name)
            .bind(self.email)
            .execute(db)
            .await?;
        User::sqlx_by_id(result.last_insert_id() as u32, db).await
    }
}

#[derive(Deserialize, Debug)]

pub struct UpdateUser {
    pub id: Option<u32>,
    pub name: Option<String>,
    pub email: Option<String>,
    pub password: Option<String>,
    // pub primary_currency: Option<Currency>,
}

impl UpdateUser {
    pub async fn sqlx_update(self, db: &sqlx::MySqlPool) -> Result<User, anyhow::Error> {
        //let _ = sqlx::query("UPDATE users SET name = ?, email = ?, password = ?, primary_currency = ? WHERE id = ?")
        let _ = sqlx::query("UPDATE users SET name = ?, email = ?, password = ? WHERE id = ?")
            .bind(&self.name)
            .bind(&self.email)
            .bind(&self.password)
            // .bind(&self.primary_currency)
            .bind(&self.id)
            .execute(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        User::sqlx_by_id(self.id.unwrap(), db).await
    }
}
