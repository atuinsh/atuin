use chrono::prelude::*;

#[derive(sqlx::FromRow)]
pub struct History {
    pub id: i64,
    pub client_id: String, // a client generated ID
    pub user_id: i64,
    pub hostname: String,
    pub timestamp: NaiveDateTime,

    pub data: String,

    pub created_at: NaiveDateTime,
}

pub struct NewHistory<'a> {
    pub client_id: &'a str,
    pub user_id: i64,
    pub hostname: &'a str,
    pub timestamp: chrono::NaiveDateTime,

    pub data: &'a str,
}

#[derive(sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub email: String,
    pub password: String,
}

#[derive(sqlx::FromRow)]
pub struct Session {
    pub id: i64,
    pub user_id: i64,
    pub token: String,
}

pub struct NewUser {
    pub username: String,
    pub email: String,
    pub password: String,
}

pub struct NewSession<'a> {
    pub user_id: i64,
    pub token: &'a str,
}
