use crate::nordigen::Requisition;
use crate::schema::*;

use cli_table::Table;

use diesel::*;
use diesel::{Associations, Identifiable, MysqlConnection, Queryable};

use serde::Serialize;

use crate::models::User;

#[derive(Table, Identifiable, Queryable, Associations, Debug, Serialize, ts_rs::TS)]
#[ts(export)]
#[diesel(belongs_to(User))]
pub struct NordigenRequisition {
    #[table(title = "Requisition ID")]
    pub id: i32,
    #[table(title = "Nordigen ID")]
    #[serde(skip_serializing)]
    pub nordigen_id: String,
    #[table(title = "Status")]
    pub status: String,
    #[table(title = "User ID")]
    #[serde(skip_serializing)]
    pub user_id: i32,
    #[table(title = "Date Created")]
    pub created_at: chrono::NaiveDateTime,
    #[table(title = "Updated At")]
    pub updated_at: chrono::NaiveDateTime,
}

pub fn create_nordigen_requisition(
    requisition: &Requisition,
    user: &User,
    con: &mut MysqlConnection,
) -> Result<(), diesel::result::Error> {
    use self::nordigen_requisitions::dsl::*;
    match insert_into(nordigen_requisitions)
        .values((
            nordigen_id.eq(requisition.id.clone()),
            status.eq(requisition.status.clone()),
            user_id.eq(user.id),
        ))
        .execute(con)
    {
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}
