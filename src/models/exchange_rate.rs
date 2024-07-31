use std::collections::HashMap;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::{mysql::MySqlRow, FromRow, MySql, Row};

use crate::ultrafinance::Currency;

#[derive(Serialize, Deserialize, Clone)]

pub struct ExchangeRate {
    pub base_code: Currency,
    pub conversion_rates: HashMap<Currency, f64>,
    pub last_update: NaiveDateTime,
}

impl<'a> FromRow<'_, MySqlRow> for ExchangeRate {
    fn from_row(row: &sqlx::mysql::MySqlRow) -> Result<ExchangeRate, sqlx::Error> {
        Ok(ExchangeRate {
            base_code: row.get("base_code"),
            conversion_rates: serde_json::from_str(row.get("conversion_rates")).map_err(|_| {
                sqlx::Error::TypeNotFound {
                    type_name: "conversion_rates".to_string(),
                }
            })?,
            last_update: row.get("last_update"),
        })
    }
}

impl ExchangeRate {
    pub async fn get_by_currency(
        currency: &Currency,
        db: &sqlx::MySqlPool,
    ) -> Result<ExchangeRate, anyhow::Error> {
        sqlx::query_as::<MySql, ExchangeRate>("SELECT * FROM exchange_rates WHERE base_code = ?")
            .bind(currency.to_string())
            .fetch_one(db)
            .await
            .map_err(|e| e.into())
    }

	pub async fn create(&self, db: &sqlx::MySqlPool) -> Result<(), anyhow::Error> {
		sqlx::query(
			"INSERT INTO exchange_rates (base_code, conversion_rates, last_update) VALUES (?, ?, ?)",
		)
		.bind(self.base_code.to_string())
		.bind(serde_json::to_string(&self.conversion_rates).unwrap())
		.bind(self.last_update)
		.execute(db)
		.await
		.map_err(|e| e.into())
		.map(|_| ())
	}

	pub async fn update(&self, db: &sqlx::MySqlPool) -> Result<(), anyhow::Error> {
		sqlx::query(
			"UPDATE exchange_rates SET conversion_rates = ?, last_update = ? WHERE base_code = ?",
		)
		.bind(serde_json::to_string(&self.conversion_rates).unwrap())
		.bind(self.last_update)
		.bind(self.base_code.to_string())
		.execute(db)
		.await
		.map_err(|e| e.into())
		.map(|_| ())
	}

	pub async fn create_or_update(&self, db: &sqlx::MySqlPool) -> Result<(), anyhow::Error> {
		match ExchangeRate::get_by_currency(&self.base_code, db).await {
			Ok(_) => self.update(db).await,
			Err(_) => self.create(db).await,
		}
	}
}
