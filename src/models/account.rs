use crate::utils::display_option;
use crate::{nordigen, schema::*};
use anyhow::Result;
use cli_table::Table;

use diesel::mysql::Mysql;
use diesel::*;
use diesel::{Associations, Identifiable, MysqlConnection, Queryable};
use paperclip::actix::Apiv2Schema;
use serde::{Serialize, Deserialize};

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
    #[table(title = "IBAN", display_fn = "display_option")]
    pub iban: Option<String>,
    #[table(title = "BIC", display_fn = "display_option")]
    pub bic: Option<String>,
    #[table(title = "BBAN", display_fn = "display_option")]
    pub bban: Option<String>,
    #[table(title = "PAN", display_fn = "display_option")]
    pub pan: Option<String>,
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

    pub fn delete(self, con: &mut MysqlConnection) -> Result<()> {
        diesel::delete(&self).execute(con)?;
        Ok(())
    }

    pub fn update(self, con: &mut MysqlConnection) -> Result<()> {
        diesel::update(&self).set(&self).execute(con)?;
        Ok(())
    }

    pub fn update_balance(&mut self) -> Result<()> {
        let mut client = nordigen::Nordigen::new();
        client.populate_token();
        let balances = client.get_account_balances(&self.nordigen_id)?;
        if balances.is_empty() {
            return Err(anyhow::anyhow!("No balances found"));
        }

        self.balance = balances.get(0).unwrap().balanceAmount.amount.parse()?;
        Ok(())
    }
}

#[derive(Insertable, Default, Debug)]
#[diesel(table_name = accounts)]
pub struct NewAccount {
    name: String,
    account_type: String,
    nordigen_id: String,
    iban: Option<String>,
    bic: Option<String>,
    bban: Option<String>,
    pan: Option<String>,
    currency: String,
    product: Option<String>,
    cash_account_type: Option<String>,
    details: String,
    owner_name: Option<String>,
    status: String,
    icon: String,
    institution_name: String,
    user_id: i32,
}

impl NewAccount {
    pub fn new(
        account_name: &str,
        nordigen_account_id: &String,
        nordigen_account_details: &nordigen::AccountDetails,
        user: &User,
    ) -> Result<Self> {
        let mut nordigen = nordigen::Nordigen::new();
        nordigen.populate_token()?;

        let nordigen_account = nordigen.get_account(nordigen_account_id)?;
        let nordigen_institution = nordigen.get_institution(&nordigen_account.institution_id)?;
        Ok(Self {
            name: account_name.to_owned(),
            account_type: "cash".into(),
            iban: nordigen_account_details.iban.clone(),
            bic: nordigen_account_details.bic.clone(),
            bban: nordigen_account_details.bban.clone(),
            pan: nordigen_account_details.pan.clone(),
            currency: nordigen_account_details.currency.clone(),
            product: nordigen_account_details.product.clone(),
            cash_account_type: nordigen_account_details.cashAccountType.clone(),
            details: nordigen_account_details
                .details
                .clone()
                .unwrap_or("".into()),
            owner_name: nordigen_account_details.ownerName.clone(),
            status: nordigen_account_details
                .status
                .clone()
                .unwrap_or("enabled".into()),
            icon: nordigen_institution.logo,
            institution_name: nordigen_institution.name,
            nordigen_id: nordigen_account_id.clone(),
            user_id: user.id,
        })
    }

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

#[derive(Deserialize, Apiv2Schema, AsChangeset, ts_rs::TS)]
#[diesel(table_name = accounts)]
#[ts(export)]
pub struct UpdateAccount {
    pub id: Option<i32>,
    pub name: Option<String>,
    pub account_type: Option<String>,
    pub iban: Option<String>,
    pub bic: Option<String>,
    pub bban: Option<String>,
    pub pan: Option<String>,
    pub currency: String,
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
            .filter(schema::accounts::id.eq(id)).first(con).map_err(|e| anyhow::anyhow!(e))
    }
}
