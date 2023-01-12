use crate::schema::{*, self};

use cli_table::Table;

use diesel::*;
use diesel::mysql::Mysql;
use diesel::{Identifiable, MysqlConnection, Queryable};
use paperclip::actix::Apiv2Schema;
use serde::{Serialize, Deserialize};

use anyhow::Result;

use crate::ultrafinance::{hash_api_key, hash_password};

#[derive(Table, Identifiable, Queryable, Debug, Serialize, ts_rs::TS, Apiv2Schema, Clone, Selectable)]
#[ts(export)]
pub struct User {
    #[table(title = "User ID")]
    pub id: i32,
    #[table(title = "Name")]
    pub name: String,
    #[table(title = "Email")]
    pub email: String,
    #[serde(skip_serializing)]
    pub(crate) password: String,
    #[table(title = "Date Created")]
    pub created_at: chrono::NaiveDateTime,
    #[table(title = "Updated At")]
    pub updated_at: chrono::NaiveDateTime,
}

type SqlType = diesel::dsl::SqlTypeOf<diesel::dsl::AsSelect<User, Mysql>>;
type BoxedQuery<'a> = crate::schema::users::BoxedQuery<'a, Mysql, SqlType>;

impl User {
    pub fn all() -> BoxedQuery<'static> {
        schema::users::table
            .select(Self::as_select())
            .into_boxed()
    }

    pub fn by_id(id: i32) -> BoxedQuery<'static> {
        Self::all()
            .filter(schema::users::id.eq(id))
    }
}


#[derive(Insertable, Default, Debug, Apiv2Schema, Deserialize, ts_rs::TS)]
#[diesel(table_name = users)]
#[ts(export)]
pub struct NewUser {
    pub name: String,
    pub email: String,
    pub password: String,
}

impl NewUser {
    pub fn create(mut self, con: &mut MysqlConnection) -> Result<User> {
        use self::users::dsl::*;
        self.password = hash_password(&self.password)?;
        match insert_into(users).values(self).execute(con) {
            Ok(_) => {
                let user_id: i32 = select(schema::last_insert_id()).first(con)?;
                let user: User = users.find(user_id).first(con)?;
                Ok(user)
            }
            Err(e) => Err(e.into()),
        }
    }
}


#[derive(AsChangeset, Deserialize, Apiv2Schema, ts_rs::TS, Debug, Identifiable)]
#[diesel(table_name = users)]
#[ts(export)]
pub struct UpdateUser {
    pub id: Option<i32>,
    pub name: Option<String>,
    pub email: Option<String>,
    pub password: Option<String>,
}

impl UpdateUser {
    pub fn update(mut self, con: &mut MysqlConnection) -> Result<()> {
        match &self.password {
            Some(p) => self.password = Some(hash_password(p.as_str())?),
            None => (),
        }

        use self::users::dsl::*;
        let update_statement = diesel::update(users)
            .filter(id.eq(self.id.ok_or(anyhow::anyhow!("No id found"))?))
            .set((&self, updated_at.eq(chrono::offset::Utc::now().naive_utc())));
        dbg!(diesel::debug_query::<Mysql, _>(&update_statement));
        update_statement.execute(con)
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(())
    }
}

pub fn create_api_key(
    user: &User,
    con: &mut MysqlConnection,
) -> Result<String, diesel::result::Error> {
    use self::user_api_keys::dsl::*;
    use uuid::Uuid;
    let raw_api_key = Uuid::new_v4().to_string();
    let ph = hash_api_key(raw_api_key.as_str());

    match insert_into(user_api_keys)
        .values((user_id.eq(user.id), api_key.eq(ph)))
        .execute(con)
    {
        Ok(_) => Ok(raw_api_key),
        Err(e) => Err(e),
    }
}
