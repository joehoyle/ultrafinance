use crate::schema::{self, *};

use cli_table::Table;

use diesel::mysql::Mysql;
use diesel::*;
use diesel::{Associations, Identifiable, MysqlConnection, Queryable};
use paperclip::actix::Apiv2Schema;
use serde::{Serialize, Deserialize};

use crate::models::User;
use anyhow::Result;

type SqlType = diesel::dsl::SqlTypeOf<diesel::dsl::AsSelect<Function, Mysql>>;
type BoxedQuery<'a> = crate::schema::functions::BoxedQuery<'a, Mysql, SqlType>;

#[derive(
    Table, Identifiable, Queryable, Associations, Serialize, Apiv2Schema, Selectable, ts_rs::TS
)]
#[diesel(belongs_to(User))]
pub struct Function {
    #[table(title = "Account ID")]
    pub id: i32,
    #[table(title = "Name")]
    pub name: String,
    #[table(title = "Type")]
    #[serde(rename = r#"type"#)]
    pub function_type: String,
    #[table(title = "Source")]
    pub source: String,
    #[table(title = "User ID")]
    #[serde(skip_serializing)]
    pub user_id: i32,
    #[table(title = "Date Created")]
    pub created_at: chrono::NaiveDateTime,
    #[table(title = "Updated At")]
    pub updated_at: chrono::NaiveDateTime,
}

impl Function {
    pub fn get_params(&self) -> anyhow::Result<crate::deno::FunctionParams> {
        let mut runtime = crate::deno::FunctionRuntime::new(self)?;
        runtime.get_params()
    }

    pub fn all() -> BoxedQuery<'static> {
        schema::functions::table
            .select(Self::as_select())
            .into_boxed()
    }

    pub fn by_user(user: &User) -> BoxedQuery<'static> {
        Self::all().filter(schema::functions::user_id.eq(user.id))
    }

    pub fn by_id(id: i32, user_id: i32) -> BoxedQuery<'static> {
        Self::all()
            .filter(schema::functions::id.eq(id))
            .filter(schema::functions::user_id.eq(user_id))
    }

    pub fn delete(self, con: &mut MysqlConnection) -> Result<()> {
        diesel::delete(&self).execute(con)?;
        Ok(())
    }
}

#[derive(Insertable, AsChangeset, Default, Debug)]
#[diesel(table_name = functions)]
pub struct NewFunction {
    pub name: String,
    pub function_type: String,
    pub source: String,
    pub user_id: i32,
}

impl NewFunction {
    pub fn create(&self, con: &mut MysqlConnection) -> Result<Function> {
        use self::functions::dsl::*;
        match insert_into(functions).values(self).execute(con) {
            Ok(_) => {
                let function_id: i32 = select(schema::last_insert_id()).first(con)?;
                let function: Function = functions.find(function_id).first(con)?;
                Ok(function)
            }
            Err(e) => Err(e.into()),
        }
    }
}


#[derive(Deserialize, Apiv2Schema, AsChangeset, ts_rs::TS)]
#[diesel(table_name = functions)]
#[ts(export)]
pub struct UpdateFunction {
    pub id: Option<i32>,
    pub name: Option<String>,
    pub source: Option<String>,
}

impl UpdateFunction {
    pub fn update(self, con: &mut MysqlConnection) -> Result<Function> {
        use self::functions::dsl::*;
        diesel::update(functions)
            .filter(id.eq(self.id.ok_or(anyhow::anyhow!("No id found"))?))
            .set((&self, updated_at.eq(chrono::offset::Utc::now().naive_utc())))
            .execute(con)
            .map_err(|e| anyhow::anyhow!(e))?;

        Function::all()
            .filter(schema::functions::id.eq(id)).first(con).map_err(|e| anyhow::anyhow!(e))
    }
}
