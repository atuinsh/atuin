use chrono::naive::NaiveDateTime;

use crate::schema::history;

#[derive(Queryable)]
pub struct History {
    pub id: String,
    pub user: String,
    pub mac: String,
    pub timestamp: NaiveDateTime,

    pub data: String,
}

#[derive(Insertable)]
#[table_name = "history"]
pub struct NewHistory<'a> {
    pub id: &'a str,
    pub user: &'a str,
    pub mac: &'a str,
    pub timestamp: &'a NaiveDateTime,

    pub data: &'a str,
}
