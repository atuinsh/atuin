use ::sqlx::{FromRow, Result};
use atuin_common::record::{EncryptedData, Record};
use atuin_server_database::models::{History, Session, User};
use sqlx::{postgres::PgRow, Row};

pub struct DbUser(pub User);
pub struct DbSession(pub Session);
pub struct DbHistory(pub History);
pub struct DbRecord(pub Record<EncryptedData>);

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

impl<'a> ::sqlx::FromRow<'a, PgRow> for DbRecord {
    fn from_row(row: &'a PgRow) -> ::sqlx::Result<Self> {
        let timestamp: i64 = row.try_get("timestamp")?;

        let data = EncryptedData {
            data: row.try_get("data")?,
            content_encryption_key: row.try_get("cek")?,
        };

        Ok(Self(Record {
            id: row.try_get("client_id")?,
            host: row.try_get("host")?,
            parent: row.try_get("parent")?,
            timestamp: timestamp as u64,
            version: row.try_get("version")?,
            tag: row.try_get("tag")?,
            data,
        }))
    }
}

impl Into<Record<EncryptedData>> for DbRecord {
    fn into(self) -> Record<EncryptedData> {
        Record { ..self.0 }
    }
}
