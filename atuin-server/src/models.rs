use chrono::prelude::*;

use atuin_common::api::EventType;

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

pub struct NewHistory {
    pub client_id: String,
    pub user_id: i64,
    pub hostname: String,
    pub timestamp: chrono::NaiveDateTime,

    pub data: String,
}

#[derive(sqlx::FromRow)]
pub struct Event {
    pub id: i64,
    pub client_id: String, // a client generated ID
    pub user_id: i64,
    pub hostname: String,
    pub timestamp: NaiveDateTime,
    pub event_type: String, // so that sqlx can unmarshal this ok 😔

    pub data: String,

    pub created_at: NaiveDateTime,
}

pub struct NewEvent {
    pub client_id: String,
    pub user_id: i64,
    pub hostname: String,
    pub timestamp: chrono::NaiveDateTime,
    pub event_type: EventType,

    pub data: String,
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

pub struct NewSession {
    pub user_id: i64,
    pub token: String,
}
