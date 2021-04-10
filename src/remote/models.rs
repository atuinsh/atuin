use chrono::prelude::*;
use chrono::Utc;

use crate::schema::{history, sessions, users};

#[derive(Deserialize, Serialize, Identifiable, Queryable, Associations)]
#[table_name = "history"]
#[belongs_to(User)]
pub struct History {
    pub id: i64,
    pub client_id: String, // a client generated ID
    pub user_id: i64,
    pub timestamp: NaiveDateTime,

    pub data: String,

    pub created_at: NaiveDateTime,
}

#[derive(Identifiable, Queryable, Associations)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub email: String,
    pub password: String,
}

#[derive(Queryable, Identifiable, Associations)]
#[belongs_to(User)]
pub struct Session {
    pub id: i64,
    pub user_id: i64,
    pub token: String,
}

#[derive(Insertable)]
#[table_name = "history"]
pub struct NewHistory<'a> {
    pub client_id: &'a str,
    pub user_id: i64,
    pub timestamp: chrono::NaiveDateTime,

    pub data: &'a str,
}

#[derive(Insertable)]
#[table_name = "users"]
pub struct NewUser<'a> {
    pub username: &'a str,
    pub email: &'a str,
    pub password: &'a str,
}

#[derive(Insertable)]
#[table_name = "sessions"]
pub struct NewSession<'a> {
    pub user_id: i64,
    pub token: &'a str,
}
