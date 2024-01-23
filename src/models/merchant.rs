use crate::utils::display_option;
use crate::schema::*;
use anyhow::Result;
use cli_table::Table;

use diesel::mysql::Mysql;
use diesel::*;
use diesel::{Identifiable, MysqlConnection, Queryable};
use paperclip::actix::Apiv2Schema;
use serde::{Deserialize, Serialize};
use diesel::deserialize::FromSql;
use diesel::serialize::{IsNull, Output, ToSql};
use diesel::sql_types::Text;
use std::error::Error;
use diesel::mysql::MysqlValue;

use crate::schema;

#[derive(
    Table,
    Identifiable,
    Queryable,
    Debug,
    Serialize,
    ts_rs::TS,
    Apiv2Schema,
    Selectable,
    Clone,
    AsChangeset,
)]
#[ts(export)]
pub struct Merchant {
    #[table(title = "Merchant ID")]
    pub id: i32,
    #[table(title = "Name")]
    pub name: String,
    #[table(skip)]
    pub logo_url: Option<String>,
    #[table(title = "Location", display_fn = "display_option")]
    pub location: Option<String>,
    #[table(skip)]
    pub location_structured: Option<Location>,
    #[table(skip)]
    pub labels: Option<String>,
    #[table(title = "External ID", display_fn = "display_option")]
    pub external_id: Option<String>,
    #[table(title = "Website", display_fn = "display_option")]
    pub website: Option<String>,
    #[table(title = "Date Created")]
    pub created_at: chrono::NaiveDateTime,
}

#[derive(
    Deserialize, Serialize, Debug, FromSqlRow, ts_rs::TS, Apiv2Schema, AsExpression, Default, Clone
)]
#[diesel(sql_type = Text)]
#[ts(export)]
pub struct Location {
    pub address: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub postcode: Option<String>,
    pub country: Option<String>,
    pub latitude: Option<f32>,
    pub longitude: Option<f32>,
    pub google_maps_url: Option<String>,
    pub apple_maps_url: Option<String>,
    pub store_number: Option<f32>,
}


impl ToSql<Text, Mysql> for Location {
    fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Mysql>) -> serialize::Result {
        serde_json::to_writer(out, self)
            .map(|_| IsNull::No)
            .map_err(|e| Box::new(e) as Box<dyn Error + Send + Sync>)
    }
}

impl FromSql<Text, Mysql> for Location {
    fn from_sql(bytes: MysqlValue<'_>) -> Result<Self, Box<dyn Error + Send + Sync>> {
        serde_json::from_slice(bytes.as_bytes())
            .map_err(|e| Box::new(e) as Box<dyn Error + Send + Sync>)
    }
}

type SqlType = diesel::dsl::SqlTypeOf<diesel::dsl::AsSelect<Merchant, Mysql>>;
type BoxedQuery<'a> = crate::schema::merchants::BoxedQuery<'a, Mysql, SqlType>;

impl Merchant {
    pub fn all() -> BoxedQuery<'static> {
        schema::merchants::table
            .select(Self::as_select())
            .into_boxed()
    }
    pub fn by_id(id: i32) -> BoxedQuery<'static> {
        Self::all()
            .filter(schema::merchants::id.eq(id))
    }

    pub fn by_external_id(id: &str) -> BoxedQuery<'static> {
        Self::all()
            .filter(schema::merchants::external_id.eq(id.to_string()))
    }

    pub fn delete(self, con: &mut MysqlConnection) -> Result<()> {
        diesel::delete(&self).execute(con)?;
        Ok(())
    }

    pub fn update(self, con: &mut MysqlConnection) -> Result<()> {
        diesel::update(&self).set(&self).execute(con)?;
        Ok(())
    }
}

#[derive(Insertable, Default, Debug)]
#[diesel(table_name = merchants)]
pub struct NewMerchant {
    pub name: String,
    pub logo_url: Option<String>,
    pub location: Option<String>,
    pub location_structured: Option<Location>,
    pub labels: Option<String>,
    pub external_id: Option<String>,
    pub website: Option<String>,
}

impl NewMerchant {
    pub fn create(self, con: &mut MysqlConnection) -> Result<Merchant> {
        use self::merchants::dsl::*;
        match insert_into(merchants).values(self).execute(con) {
            Ok(_) => {
                let merchant_id: i32 = select(schema::last_insert_id()).first(con)?;
                let merchant: Merchant = merchants.find(merchant_id).first(con)?;
                Ok(merchant)
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn create_or_fetch(self, con: &mut MysqlConnection) -> Result<Merchant> {
        match &self.external_id {
            Some(external_id) => {
                match Merchant::by_external_id(external_id).first(con) {
                    Ok(merchant) => Ok(merchant),
                    Err(_) => {
                        let merchant = self.create(con)?;
                        Ok(merchant)
                    }
                }
            }
            None => {
                let merchant = self.create(con)?;
                Ok(merchant)
            }
        }
    }
}
