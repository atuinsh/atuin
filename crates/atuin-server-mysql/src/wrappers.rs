use atuin_common::record::{EncryptedData, Host, Record};
use atuin_server_database::models::{History, Session, User};
use sqlx::{FromRow, Result, Row, mysql::MySqlRow};
use time::PrimitiveDateTime;

pub struct DbUser(pub User);
pub struct DbSession(pub Session);
pub struct DbHistory(pub History);
pub struct DbRecord(pub Record<EncryptedData>);

impl<'a> FromRow<'a, MySqlRow> for DbUser {
    fn from_row(row: &'a MySqlRow) -> Result<Self> {
        Ok(Self(User {
            id: row.try_get("id")?,
            username: row.try_get("username")?,
            email: row.try_get("email")?,
            password: row.try_get("password")?,
        }))
    }
}

impl<'a> ::sqlx::FromRow<'a, MySqlRow> for DbSession {
    fn from_row(row: &'a MySqlRow) -> ::sqlx::Result<Self> {
        Ok(Self(Session {
            id: row.try_get("id")?,
            user_id: row.try_get("user_id")?,
            token: row.try_get("token")?,
        }))
    }
}

impl<'a> ::sqlx::FromRow<'a, MySqlRow> for DbHistory {
    fn from_row(row: &'a MySqlRow) -> ::sqlx::Result<Self> {
        Ok(Self(History {
            id: row.try_get("id")?,
            client_id: row.try_get("client_id")?,
            user_id: row.try_get("user_id")?,
            hostname: row.try_get("hostname")?,
            timestamp: row
                .try_get::<PrimitiveDateTime, _>("timestamp")?
                .assume_utc(),
            data: row.try_get("data")?,
            created_at: row
                .try_get::<PrimitiveDateTime, _>("created_at")?
                .assume_utc(),
        }))
    }
}

impl<'a> ::sqlx::FromRow<'a, MySqlRow> for DbRecord {
    fn from_row(row: &'a MySqlRow) -> ::sqlx::Result<Self> {
        let timestamp: i64 = row.try_get("timestamp")?;
        let idx: i64 = row.try_get("idx")?;

        let data = EncryptedData {
            data: row.try_get("data")?,
            content_encryption_key: row.try_get("cek")?,
        };

        let client_id_bytes: Vec<u8> = row.try_get("client_id")?;
        let client_id = uuid::Uuid::from_slice(&client_id_bytes)
            .map_err(|e| ::sqlx::Error::Decode(Box::new(e)))?;

        let host_bytes: Vec<u8> = row.try_get("host")?;
        let host_uuid =
            uuid::Uuid::from_slice(&host_bytes).map_err(|e| ::sqlx::Error::Decode(Box::new(e)))?;

        Ok(Self(Record {
            id: atuin_common::record::RecordId(client_id),
            host: Host::new(atuin_common::record::HostId(host_uuid)),
            idx: idx as u64,
            timestamp: timestamp as u64,
            version: row.try_get("version")?,
            tag: row.try_get("tag")?,
            data,
        }))
    }
}

impl From<DbRecord> for Record<EncryptedData> {
    fn from(other: DbRecord) -> Record<EncryptedData> {
        Record { ..other.0 }
    }
}
