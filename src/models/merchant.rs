use crate::utils::display_option;
use cli_table::Table;

use paperclip::actix::Apiv2Schema;
use serde::{Deserialize, Serialize};

#[derive(Table, Debug, Serialize, ts_rs::TS, Apiv2Schema, Clone)]
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

#[derive(Deserialize, Serialize, Debug, ts_rs::TS, Apiv2Schema, Default, Clone)]
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

impl TryFrom<String> for Location {
    type Error = anyhow::Error;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        serde_json::from_str(s.as_str()).map_err(|e| anyhow::anyhow!(e))
    }
}

impl Merchant {
    pub async fn sqlx_all(db: &sqlx::MySqlPool) -> Result<Vec<Self>, anyhow::Error> {
        let query_args =
            <sqlx::mysql::MySql as ::sqlx::database::HasArguments>::Arguments::default();
        ::sqlx::query_with::<sqlx::mysql::MySql, _>("SELECT * FROM merchants", query_args)
            .try_map(Self::map_result)
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
        .try_map(Self::map_result)
        .fetch_one(db)
        .await
        .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_name_by_website(name: &str, website: &str, db: &sqlx::MySqlPool) -> Result<Self, anyhow::Error> {
        use ::sqlx::Arguments as _;
        let arg0 = &(name);
        let arg1 = &(website);
        let mut query_args =
            <sqlx::mysql::MySql as ::sqlx::database::HasArguments>::Arguments::default();
        query_args.reserve(
            1usize,
            0 + ::sqlx::encode::Encode::<sqlx::mysql::MySql>::size_hint(arg0) + ::sqlx::encode::Encode::<sqlx::mysql::MySql>::size_hint(arg1),
        );
        query_args.add(arg0);
        query_args.add(arg1);
        ::sqlx::query_with::<sqlx::mysql::MySql, _>(
            "SELECT * FROM merchants WHERE name = ? AND website = ?",
            query_args,
        )
        .try_map(Self::map_result)
        .fetch_one(db)
        .await
        .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn sqlx_by_external_id(
        external_id: &String,
        db: &sqlx::MySqlPool,
    ) -> Result<Self, anyhow::Error> {
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
        .try_map(Self::map_result)
        .fetch_one(db)
        .await
        .map_err(|e| anyhow::anyhow!(e))
    }

    fn map_result(row: sqlx::mysql::MySqlRow) -> Result<Self, sqlx::Error> {
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
                .and_then(|a| a.try_into().ok());
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
    }
}

#[derive(Default, Debug, Deserialize, Clone)]
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
    pub async fn sqlx_create(self, db: &sqlx::MySqlPool) -> Result<Merchant, anyhow::Error> {
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

    pub async fn sqlx_create_or_fetch(self, db: &sqlx::MySqlPool) -> Result<Merchant, anyhow::Error> {
        match &self.external_id {
            Some(external_id) => match Merchant::sqlx_by_external_id(external_id, db).await {
                Ok(merchant) => Ok(merchant),
                Err(_) => {
                    match Merchant::sqlx_by_name_by_website(&self.name, &self.website.as_ref().unwrap(), db).await {
                        Ok(merchant) => Ok(merchant),
                        Err(_) => {
                            self.sqlx_create(db).await
                        }
                    }
                }
            },
            None => {
                // Fall back to getting it by name and URL
                match &self.website {
                    Some(website) => match Merchant::sqlx_by_name_by_website(&self.name, website, db).await {
                        Ok(merchant) => Ok(merchant),
                        Err(_) => self.sqlx_create(db).await,
                    },
                    None => self.sqlx_create(db).await,
                }
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
