use cli_table::Table;

use paperclip::actix::Apiv2Schema;
use serde::{Deserialize, Serialize};

use anyhow::Result;

#[derive(Table, Serialize, Apiv2Schema, ts_rs::TS, sqlx::FromRow)]
pub struct Function {
    #[table(title = "Account ID")]
    pub id: u32,
    #[table(title = "Name")]
    pub name: String,
    #[table(title = "Type")]
    #[serde(rename = r#"type"#)]
    pub function_type: String,
    #[table(title = "Source")]
    pub source: String,
    #[table(title = "User ID")]
    #[serde(skip_serializing)]
    pub user_id: u32,
    #[table(title = "Date Created")]
    pub created_at: chrono::NaiveDateTime,
    #[table(title = "Updated At")]
    pub updated_at: chrono::NaiveDateTime,
}

impl Function {
    pub async fn get_params(&self) -> anyhow::Result<crate::deno::FunctionParams> {
        let mut runtime = crate::deno::FunctionRuntime::new(self).await?;
        runtime.get_params()
    }

    pub async fn sqlx_all(db: &sqlx::MySqlPool) -> Result<Vec<Self>, anyhow::Error> {
        sqlx::QueryBuilder::new("SELECT * FROM functions")
            .build_query_as::<Self>()
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_id(id: u32, db: &sqlx::MySqlPool) -> Result<Self, anyhow::Error> {
        sqlx::query_as::<_, Self>("SELECT * FROM functions WHERE id = ?")
            .bind(id)
            .fetch_one(db)
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
            "SELECT * FROM functions WHERE id = ? AND user_id = ?",
            id,
            user_id
        )
        .fetch_one(db)
        .await
        .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_user(
        user_id: u32,
        db: &sqlx::MySqlPool,
    ) -> Result<Vec<Self>, anyhow::Error> {
        sqlx::query_as!(Self, "SELECT * FROM functions WHERE user_id = ?", user_id)
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_delete(&self, db: &sqlx::MySqlPool) -> Result<(), anyhow::Error> {
        sqlx::query!("DELETE FROM accounts WHERE id = ?", &self.id)
            .execute(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(())
    }
}

#[derive(Default, Debug)]
pub struct NewFunction {
    pub name: String,
    pub function_type: String,
    pub source: String,
    pub user_id: u32,
}

impl NewFunction {
    pub async fn sqlx_create(self, db: &sqlx::MySqlPool) -> Result<Function, anyhow::Error> {
        let result = sqlx::query!(
            "INSERT INTO functions (name, function_type, source, user_id) VALUES (?, ?, ?, ?)",
            self.name,
            self.function_type,
            self.source,
            self.user_id
        )
        .execute(db)
        .await?;

        Function::sqlx_by_id(result.last_insert_id() as u32, db).await
    }
}

#[derive(Deserialize, Apiv2Schema, ts_rs::TS)]
#[ts(export)]
pub struct UpdateFunction {
    pub id: Option<u32>,
    pub name: Option<String>,
    pub source: Option<String>,
}

impl UpdateFunction {
    pub async fn sqlx_update(self, db: &sqlx::MySqlPool) -> Result<Function, anyhow::Error> {
        let id = self.id.ok_or(anyhow::anyhow!("No id found"))?;
        sqlx::query!(
            "UPDATE functions SET name = ?, source = ? WHERE id = ?",
            self.name,
            self.source,
            id
        )
        .execute(db)
        .await?;
        Function::sqlx_by_id(id, db).await
    }
}
