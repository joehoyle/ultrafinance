use crate::schema::*;
use crate::utils::display_option;
use anyhow::Result;
use cli_table::Table;

use diesel::deserialize::FromSql;
use diesel::mysql::Mysql;
use diesel::mysql::MysqlValue;
use diesel::serialize::{IsNull, Output, ToSql};
use diesel::sql_types::Text;
use diesel::*;
use diesel::{Identifiable, MysqlConnection, Queryable};
use paperclip::actix::Apiv2Schema;
use serde::{Deserialize, Serialize};
use std::error::Error;

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
    pub id: u32,
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
    Deserialize, Serialize, Debug, FromSqlRow, ts_rs::TS, Apiv2Schema, AsExpression, Default, Clone,
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

impl From<String> for Location {
    fn from(s: String) -> Self {
        serde_json::from_str(s.as_str()).unwrap()
    }
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
    pub fn by_id(id: u32) -> BoxedQuery<'static> {
        Self::all().filter(schema::merchants::id.eq(id))
    }

    pub fn by_external_id(id: &str) -> BoxedQuery<'static> {
        Self::all().filter(schema::merchants::external_id.eq(id.to_string()))
    }

    pub fn delete(self, con: &mut MysqlConnection) -> Result<()> {
        diesel::delete(&self).execute(con)?;
        Ok(())
    }

    pub fn update(self, con: &mut MysqlConnection) -> Result<()> {
        diesel::update(&self).set(&self).execute(con)?;
        Ok(())
    }

    pub async fn sqlx_all(db: &sqlx::MySqlPool) -> Result<Vec<Self>, anyhow::Error> {
        let query_args =
            <sqlx::mysql::MySql as ::sqlx::database::HasArguments>::Arguments::default();
        ::sqlx::query_with::<sqlx::mysql::MySql, _>("SELECT * FROM merchants", query_args)
            .try_map(|row: sqlx::mysql::MySqlRow| {
                use ::sqlx::Row as _;
                let sqlx_query_as_id = row.try_get_unchecked::<u32, _>(0usize)?.into();
                let sqlx_query_as_name = row.try_get_unchecked::<String, _>(1usize)?.into();
                let sqlx_query_as_logo_url = row
                    .try_get_unchecked::<::std::option::Option<String>, _>(2usize)?
                    .into();
                let sqlx_query_as_location = row
                    .try_get_unchecked::<::std::option::Option<String>, _>(3usize)?
                    .into();
                let sqlx_query_as_location_structured = row
                    .try_get_unchecked::<::std::option::Option<String>, _>(4usize)?
                    .map(|a| a.into());
                let sqlx_query_as_labels = row
                    .try_get_unchecked::<::std::option::Option<String>, _>(5usize)?
                    .into();
                let sqlx_query_as_external_id = row
                    .try_get_unchecked::<::std::option::Option<String>, _>(6usize)?
                    .into();
                let sqlx_query_as_website = row
                    .try_get_unchecked::<::std::option::Option<String>, _>(7usize)?
                    .into();
                let sqlx_query_as_created_at = row
                    .try_get_unchecked::<sqlx::types::chrono::NaiveDateTime, _>(8usize)?
                    .into();
                ::std::result::Result::Ok(Merchant {
                    r#id: sqlx_query_as_id,
                    r#name: sqlx_query_as_name,
                    r#logo_url: sqlx_query_as_logo_url,
                    r#location: sqlx_query_as_location,
                    r#location_structured: sqlx_query_as_location_structured,
                    r#labels: sqlx_query_as_labels,
                    r#external_id: sqlx_query_as_external_id,
                    r#website: sqlx_query_as_website,
                    r#created_at: sqlx_query_as_created_at,
                })
            })
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_id(id: u32, db: &sqlx::MySqlPool) -> Result<Self, anyhow::Error> {
        use ::sqlx::Arguments as _;
        let arg0 = &(id);
        let mut query_args =
            <sqlx::mysql::MySql as ::sqlx::database::HasArguments>::Arguments::default();
        query_args.reserve(
            1usize,
            0 + ::sqlx::encode::Encode::<sqlx::mysql::MySql>::size_hint(arg0),
        );
        query_args.add(arg0);
        ::sqlx::query_with::<sqlx::mysql::MySql, _>(
            "SELECT * FROM merchants WHERE id = ?",
            query_args,
        )
        .try_map(|row: sqlx::mysql::MySqlRow| {
            use ::sqlx::Row as _;
            let sqlx_query_as_id = row.try_get_unchecked::<u32, _>(0usize)?.into();
            let sqlx_query_as_name = row.try_get_unchecked::<String, _>(1usize)?.into();
            let sqlx_query_as_logo_url = row
                .try_get_unchecked::<::std::option::Option<String>, _>(2usize)?
                .into();
            let sqlx_query_as_location = row
                .try_get_unchecked::<::std::option::Option<String>, _>(3usize)?
                .into();
            let sqlx_query_as_location_structured = row
                .try_get_unchecked::<::std::option::Option<String>, _>(4usize)?
                .map(|a| a.into());
            let sqlx_query_as_labels = row
                .try_get_unchecked::<::std::option::Option<String>, _>(5usize)?
                .into();
            let sqlx_query_as_external_id = row
                .try_get_unchecked::<::std::option::Option<String>, _>(6usize)?
                .into();
            let sqlx_query_as_website = row
                .try_get_unchecked::<::std::option::Option<String>, _>(7usize)?
                .into();
            let sqlx_query_as_created_at = row
                .try_get_unchecked::<sqlx::types::chrono::NaiveDateTime, _>(8usize)?
                .into();
            ::std::result::Result::Ok(Self {
                r#id: sqlx_query_as_id,
                r#name: sqlx_query_as_name,
                r#logo_url: sqlx_query_as_logo_url,
                r#location: sqlx_query_as_location,
                r#location_structured: sqlx_query_as_location_structured,
                r#labels: sqlx_query_as_labels,
                r#external_id: sqlx_query_as_external_id,
                r#website: sqlx_query_as_website,
                r#created_at: sqlx_query_as_created_at,
            })
        })
        .fetch_one(db)
        .await
        .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_external_id(external_id: &String, db: &sqlx::MySqlPool) -> Result<Self, anyhow::Error> {
        use ::sqlx::Arguments as _;
        let arg0 = &(external_id);
        let mut query_args =
            <sqlx::mysql::MySql as ::sqlx::database::HasArguments>::Arguments::default();
        query_args.reserve(
            1usize,
            0 + ::sqlx::encode::Encode::<sqlx::mysql::MySql>::size_hint(arg0),
        );
        query_args.add(arg0);
        ::sqlx::query_with::<sqlx::mysql::MySql, _>(
            "SELECT * FROM merchants WHERE external_id = ?",
            query_args,
        )
        .try_map(|row: sqlx::mysql::MySqlRow| {
            use ::sqlx::Row as _;
            let sqlx_query_as_id = row.try_get_unchecked::<u32, _>(0usize)?.into();
            let sqlx_query_as_name = row.try_get_unchecked::<String, _>(1usize)?.into();
            let sqlx_query_as_logo_url = row
                .try_get_unchecked::<::std::option::Option<String>, _>(2usize)?
                .into();
            let sqlx_query_as_location = row
                .try_get_unchecked::<::std::option::Option<String>, _>(3usize)?
                .into();
            let sqlx_query_as_location_structured = row
                .try_get_unchecked::<::std::option::Option<String>, _>(4usize)?
                .map(|a| a.into());
            let sqlx_query_as_labels = row
                .try_get_unchecked::<::std::option::Option<String>, _>(5usize)?
                .into();
            let sqlx_query_as_external_id = row
                .try_get_unchecked::<::std::option::Option<String>, _>(6usize)?
                .into();
            let sqlx_query_as_website = row
                .try_get_unchecked::<::std::option::Option<String>, _>(7usize)?
                .into();
            let sqlx_query_as_created_at = row
                .try_get_unchecked::<sqlx::types::chrono::NaiveDateTime, _>(8usize)?
                .into();
            ::std::result::Result::Ok(Self {
                r#id: sqlx_query_as_id,
                r#name: sqlx_query_as_name,
                r#logo_url: sqlx_query_as_logo_url,
                r#location: sqlx_query_as_location,
                r#location_structured: sqlx_query_as_location_structured,
                r#labels: sqlx_query_as_labels,
                r#external_id: sqlx_query_as_external_id,
                r#website: sqlx_query_as_website,
                r#created_at: sqlx_query_as_created_at,
            })
        })
        .fetch_one(db)
        .await
        .map_err(|e| anyhow::anyhow!(e))
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
                let merchant_id: u32 = select(schema::last_insert_id()).first(con)?;
                let merchant: Merchant = merchants.find(merchant_id).first(con)?;
                Ok(merchant)
            }
            Err(e) => Err(e.into()),
        }
    }

    pub async fn sqlx_create(self, db: &sqlx::MySqlPool) -> Result<Merchant> {
        let result = sqlx::query!(
            "INSERT INTO merchants (name, logo_url, location, location_structured, labels, external_id, website) VALUES (?, ?, ?, ?, ?, ?, ?)",
            self.name,
            self.logo_url,
            self.location,
            serde_json::json!(self.location_structured).to_string(),
            self.labels,
            self.external_id,
            self.website
        ) .execute(db)
            .await?;
        Merchant::sqlx_by_id(result.last_insert_id() as u32, db).await
    }

    pub fn create_or_fetch(self, con: &mut MysqlConnection) -> Result<Merchant> {
        match &self.external_id {
            Some(external_id) => match Merchant::by_external_id(external_id).first(con) {
                Ok(merchant) => Ok(merchant),
                Err(_) => {
                    let merchant = self.create(con)?;
                    Ok(merchant)
                }
            },
            None => {
                let merchant = self.create(con)?;
                Ok(merchant)
            }
        }
    }

    pub async fn sqlx_create_or_fetch(self, db: &sqlx::MySqlPool) -> Result<Merchant> {
        match &self.external_id {
            Some(external_id) => match Merchant::sqlx_by_external_id(external_id, db).await {
                Ok(merchant) => Ok(merchant),
                Err(_) => {
                    let merchant = self.sqlx_create(db).await?;
                    Ok(merchant)
                }
            },
            None => {
                let merchant = self.sqlx_create(db).await?;
                Ok(merchant)
            }
        }
    }

    pub async fn sqlx_create_of_fetch(
        self,
        db: &sqlx::MySqlPool,
    ) -> Result<Merchant, anyhow::Error> {
        match &self.external_id {
            Some(external_id) => match Merchant::sqlx_by_external_id(external_id, db).await {
                Ok(merchant) => Ok(merchant),
                Err(_) => {
                    let merchant = self.sqlx_create(db).await?;
                    Ok(merchant)
                }
            },
            None => {
                let merchant = self.sqlx_create(db).await?;
                Ok(merchant)
            }
        }
    }
}
