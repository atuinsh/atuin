use std::borrow::Cow;

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
    pub client_id: Cow<'a, str>,
    pub user_id: i64,
    pub hostname: Cow<'a, str>,
    pub timestamp: chrono::NaiveDateTime,

    pub data: Cow<'a, str>,
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

pub struct NewUser<'a> {
    pub username: Cow<'a, str>,
    pub email: Cow<'a, str>,
    pub password: Cow<'a, str>,
}

pub struct NewSession<'a> {
    pub user_id: i64,
    pub token: Cow<'a, str>,
}
