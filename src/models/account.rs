use crate::accounts::{get_source_account, SourceAccount, SourceAccountDetails};
use crate::utils::display_option;
use anyhow::Result;
use cli_table::Table;

use paperclip::actix::Apiv2Schema;
use serde::{Deserialize, Serialize};

#[derive(Table, Debug, Serialize, ts_rs::TS, Apiv2Schema, Clone)]
#[ts(export)]
#[derive(sqlx::FromRow)]
pub struct Account {
    #[table(title = "Account ID")]
    pub id: u32,
    #[table(title = "Name")]
    pub name: String,
    #[table(title = "Type")]
    pub account_type: String,
    pub currency: String,
    #[table(title = "Product", display_fn = "display_option")]
    pub product: Option<String>,
    #[table(title = "Cash Account Type", display_fn = "display_option")]
    pub cash_account_type: Option<String>,
    #[ts(inline)]
    pub status: String,
    pub details: String,
    pub balance: f32,
    #[table(title = "Owner Name", display_fn = "display_option")]
    pub owner_name: Option<String>,
    #[table(skip)]
    pub icon: Option<String>,
    pub institution_name: String,
    #[table(title = "Nordigen ID")]
    #[serde(skip_serializing)]
    pub nordigen_id: String,
    #[table(title = "User ID")]
    #[serde(skip_serializing)]
    pub user_id: u32,
    #[table(title = "Date Created")]
    pub created_at: chrono::NaiveDateTime,
    #[table(title = "Updated At")]
    pub updated_at: chrono::NaiveDateTime,
    #[table(title = "Config", display_fn = "display_option")]
    pub config: Option<String>,
    #[table(title = "Number", display_fn = "display_option")]
    pub number: Option<String>,
}

impl Account {
    pub async fn update_balance(&mut self) -> Result<()> {
        let balance = self.source()?.balance().await?;

        self.balance = balance.amount.parse::<f32>()?;
        Ok(())
    }

    pub fn source(&self) -> Result<Box<impl SourceAccount>> {
        let config = match &self.config {
            Some(config) => config,
            None => return Err(anyhow::anyhow!("No config found")),
        };
        get_source_account(&self.account_type, config).ok_or(anyhow::anyhow!("No source found"))
    }

    pub async fn sqlx_all(db: &sqlx::MySqlPool) -> Result<Vec<Self>, anyhow::Error> {
        sqlx::QueryBuilder::new("SELECT * FROM accounts")
            .build_query_as::<Self>()
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_id(
        id: u32,
        user_id: u32,
        db: &sqlx::MySqlPool,
    ) -> Result<Account, anyhow::Error> {
        sqlx::query_as::<_, Account>("SELECT * FROM accounts WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .fetch_one(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_user(
        user_id: u32,
        db: &sqlx::MySqlPool,
    ) -> Result<Vec<Account>, anyhow::Error> {
        sqlx::query_as::<_, Account>("SELECT * FROM accounts WHERE user_id = ?")
            .bind(user_id)
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_id_only(id: u32, db: &sqlx::MySqlPool) -> Result<Account, anyhow::Error> {
        sqlx::query_as::<_, Account>("SELECT * FROM accounts WHERE id = ?")
            .bind(id)
            .fetch_one(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_source_account_details(
        details: SourceAccountDetails,
        user_id: u32,
        db: &sqlx::MySqlPool,
    ) -> Result<Account, anyhow::Error> {
        sqlx::query_as::<_, Account>(
            "SELECT * FROM accounts WHERE number = ? AND institution_name = ? AND user_id = ?",
        )
        .bind(details.number)
        .bind(details.institution_name)
        .bind(user_id)
        .fetch_one(db)
        .await
        .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_update(&mut self, db: &sqlx::MySqlPool) -> Result<Account, anyhow::Error> {
        self.updated_at = chrono::Local::now().naive_local();
        sqlx::query("UPDATE accounts SET name = ?, number = ?, account_type = ?, nordigen_id = ?, currency = ?, product = ?, cash_account_type = ?, details = ?, owner_name = ?, status = ?, icon = ?, institution_name = ?, config = ?, user_id = ?, updated_at = ? WHERE id = ?")
            .bind(&self.name)
            .bind(&self.number)
            .bind(&self.account_type)
            .bind(&self.nordigen_id)
            .bind(&self.currency)
            .bind(&self.product)
            .bind(&self.cash_account_type)
            .bind(&self.details)
            .bind(&self.owner_name)
            .bind(&self.status)
            .bind(&self.icon)
            .bind(&self.institution_name)
            .bind(&self.config)
            .bind(&self.user_id)
            .bind(&self.updated_at)
            .bind(&self.id)
            .execute(db)
            .await?;
        Account::sqlx_by_id_only(self.id, db).await
    }

    pub async fn sqlx_delete(&self, db: &sqlx::MySqlPool) -> Result<(), anyhow::Error> {
        sqlx::query("DELETE FROM accounts WHERE id = ?")
            .bind(&self.id)
            .execute(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(())
    }
}

pub struct NewAccount {
    pub name: String,
    pub number: Option<String>,
    pub account_type: String,
    pub nordigen_id: String,
    pub currency: String,
    pub product: Option<String>,
    pub cash_account_type: Option<String>,
    pub details: String,
    pub owner_name: Option<String>,
    pub status: String,
    pub icon: String,
    pub institution_name: String,
    pub config: Option<String>,
    pub user_id: u32,
}

impl NewAccount {
    pub async fn sqlx_create(self, db: &sqlx::MySqlPool) -> Result<Account, anyhow::Error> {
        let result = sqlx::query!("INSERT INTO accounts (name, number, account_type, nordigen_id, currency, product, cash_account_type, details, owner_name, status, icon, institution_name, config, user_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            self.name,
            self.number,
            self.account_type,
            self.nordigen_id,
            self.currency,
            self.product,
            self.cash_account_type,
            self.details,
            self.owner_name,
            self.status,
            self.icon,
            self.institution_name,
            self.config,
            self.user_id
    )
            .execute(db)
            .await?;
        Account::sqlx_by_id(result.last_insert_id() as u32, self.user_id, db).await
    }
}

impl From<SourceAccountDetails> for NewAccount {
    fn from(details: SourceAccountDetails) -> Self {
        Self {
            name: "".to_string(),
            number: Some(details.number),
            account_type: "".into(),
            currency: details.currency,
            product: None,
            cash_account_type: None,
            details: details.details,
            owner_name: details.owner_name,
            status: "active".into(),
            icon: details.icon.unwrap_or("".into()),
            institution_name: details.institution_name,
            nordigen_id: "".into(),
            user_id: 0,
            config: None,
        }
    }
}

#[derive(ts_rs::TS, Deserialize, Apiv2Schema)]
#[ts(export)]
pub struct UpdateAccount {
    pub id: Option<u32>,
    pub number: Option<String>,
    pub name: Option<String>,
    pub account_type: Option<String>,
    pub currency: Option<String>,
    pub product: Option<String>,
    pub cash_account_type: Option<String>,
    pub details: Option<String>,
    pub owner_name: Option<String>,
    pub status: Option<String>,
    pub icon: Option<String>,
    pub institution_name: Option<String>,
}

impl UpdateAccount {
    pub async fn sqlx_update(self, db: &sqlx::MySqlPool) -> Result<Account, anyhow::Error> {
        let result = sqlx::query("UPDATE accounts SET name = ?, number = ?, account_type = ?, nordigen_id = ?, currency = ?, product = ?, cash_account_type = ?, details = ?, owner_name = ?, status = ?, icon = ?, institution_name = ?, config = ?, user_id = ?, updated_at = ? WHERE id = ?")
            .bind(&self.name)
            .bind(&self.number)
            .bind(&self.account_type)
            .bind(&self.currency)
            .bind(&self.product)
            .bind(&self.cash_account_type)
            .bind(&self.details)
            .bind(&self.owner_name)
            .bind(&self.status)
            .bind(&self.icon)
            .bind(&self.institution_name)
            .bind(&self.id)
            .execute(db)
            .await?;
        Account::sqlx_by_id_only(result.last_insert_id() as u32, db).await
    }
}

impl From<SourceAccountDetails> for UpdateAccount {
    fn from(details: SourceAccountDetails) -> Self {
        Self {
            id: None,
            name: None,
            number: Some(details.number),
            account_type: None,
            currency: Some(details.currency),
            product: None,
            cash_account_type: None,
            details: Some(details.details),
            owner_name: details.owner_name,
            status: None,
            icon: details.icon,
            institution_name: Some(details.institution_name),
        }
    }
}
