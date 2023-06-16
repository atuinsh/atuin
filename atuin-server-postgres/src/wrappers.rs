use ::sqlx::{FromRow, Result};
use atuin_server_database::models::{History, Session, User};
use sqlx::{postgres::PgRow, Row};

pub struct DbUser(pub User);
pub struct DbSession(pub Session);
pub struct DbHistory(pub History);

impl<'a> FromRow<'a, PgRow> for DbUser {
    fn from_row(row: &'a PgRow) -> Result<Self> {
        Ok(Self(User {
            id: row.try_get("id")?,
            username: row.try_get("username")?,
            email: row.try_get("email")?,
            password: row.try_get("password")?,
        }))
    }
}

impl<'a> ::sqlx::FromRow<'a, PgRow> for DbSession {
    fn from_row(row: &'a PgRow) -> ::sqlx::Result<Self> {
        Ok(Self(Session {
            id: row.try_get("id")?,
            user_id: row.try_get("user_id")?,
            token: row.try_get("token")?,
        }))
    }
}

impl<'a> ::sqlx::FromRow<'a, PgRow> for DbHistory {
    fn from_row(row: &'a PgRow) -> ::sqlx::Result<Self> {
        Ok(Self(History {
            id: row.try_get("id")?,
            client_id: row.try_get("client_id")?,
            user_id: row.try_get("user_id")?,
            hostname: row.try_get("hostname")?,
            timestamp: row.try_get("timestamp")?,
            data: row.try_get("data")?,
            created_at: row.try_get("created_at")?,
        }))
    }
}
