use crate::accounts::{get_source_account, SourceAccount, SourceAccountDetails};
use crate::utils::display_option;
use crate::schema::*;
use anyhow::Result;
use cli_table::Table;

use diesel::mysql::Mysql;
use diesel::*;
use diesel::{Associations, Identifiable, MysqlConnection, Queryable};
use paperclip::actix::Apiv2Schema;
use serde::{Deserialize, Serialize};

use crate::models::User;
use crate::schema;

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
    Clone,
    AsChangeset,
)]
#[ts(export)]
#[diesel(belongs_to(User))]
pub struct Account {
    #[table(title = "Account ID")]
    pub id: i32,
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
    pub user_id: i32,
    #[table(title = "Date Created")]
    pub created_at: chrono::NaiveDateTime,
    #[table(title = "Updated At")]
    pub updated_at: chrono::NaiveDateTime,
    #[table(title = "Config", display_fn = "display_option")]
    pub config: Option<String>,
    #[table(title = "Number", display_fn = "display_option")]
    pub number: Option<String>,
}

type SqlType = diesel::dsl::SqlTypeOf<diesel::dsl::AsSelect<Account, Mysql>>;
type BoxedQuery<'a> = crate::schema::accounts::BoxedQuery<'a, Mysql, SqlType>;

impl Account {
    pub fn all() -> BoxedQuery<'static> {
        schema::accounts::table
            .select(Self::as_select())
            .into_boxed()
    }

    pub fn by_user(user: &User) -> BoxedQuery<'static> {
        Self::all().filter(schema::accounts::user_id.eq(user.id))
    }

    pub fn by_id(id: i32, user_id: i32) -> BoxedQuery<'static> {
        Self::all()
            .filter(schema::accounts::id.eq(id))
            .filter(schema::accounts::user_id.eq(user_id))
    }

    pub fn by_source_account_details(
        details: SourceAccountDetails,
        user_id: i32,
    ) -> BoxedQuery<'static> {
        Self::all()
            .filter(schema::accounts::user_id.eq(user_id))
            .filter(schema::accounts::number.eq(details.number))
            .filter(schema::accounts::institution_name.eq(details.institution_name))
    }

    pub fn delete(self, con: &mut MysqlConnection) -> Result<()> {
        diesel::delete(&self).execute(con)?;
        Ok(())
    }

    pub fn update(&mut self, con: &mut MysqlConnection) -> Result<()> {
        self.updated_at = chrono::Local::now().naive_local();
        diesel::update(&*self).set(&*self).execute(con)?;
        Ok(())
    }

    pub fn update_balance(&mut self) -> Result<()> {
        let balance = self.source()?.balance()?;

        self.balance = balance.amount.parse::<f32>()?;
        Ok(())
    }

    pub fn source(&self) -> Result<Box<dyn SourceAccount>> {
        let config = match &self.config {
            Some(config) => config,
            None => return Err(anyhow::anyhow!("No config found")),
        };
        get_source_account(&self.account_type, config).ok_or(anyhow::anyhow!("No source found"))
    }
}

#[derive(Insertable, Default, Debug)]
#[diesel(table_name = accounts)]
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
    pub user_id: i32,
}

impl NewAccount {
    pub fn create(self, con: &mut MysqlConnection) -> Result<Account> {
        use self::accounts::dsl::*;
        match insert_into(accounts).values(self).execute(con) {
            Ok(_) => {
                let account_id: i32 = select(schema::last_insert_id()).first(con)?;
                let account: Account = accounts.find(account_id).first(con)?;
                Ok(account)
            }
            Err(e) => Err(e.into()),
        }
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

#[derive(Deserialize, Apiv2Schema, AsChangeset, ts_rs::TS)]
#[diesel(table_name = accounts)]
#[ts(export)]
pub struct UpdateAccount {
    pub id: Option<i32>,
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
    pub fn update(self, con: &mut MysqlConnection) -> Result<Account> {
        use self::accounts::dsl::*;
        diesel::update(accounts)
            .filter(id.eq(self.id.ok_or(anyhow::anyhow!("No id found"))?))
            .set((&self, updated_at.eq(chrono::offset::Utc::now().naive_utc())))
            .execute(con)
            .map_err(|e| anyhow::anyhow!(e))?;

        Account::all()
            .filter(schema::accounts::id.eq(id))
            .first(con)
            .map_err(|e| anyhow::anyhow!(e))
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
