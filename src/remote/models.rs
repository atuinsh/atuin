use chrono::naive::NaiveDateTime;

use crate::schema::{history, users};

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

#[derive(Queryable)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub api: String,
    pub key: String,
}

#[derive(Insertable)]
#[table_name = "users"]
pub struct NewUser<'a> {
    pub email: &'a str,
    pub key: &'a str,
    pub api: &'a str,
}
