use atuin_domain::record::{
    EncryptedData, Host, HostId, Record, RecordIdx, RecordSeriesKey, RecordTag, RecordVersion,
};
use sqlx::Row;

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

#[derive(derive_more::Into)]
pub(super) struct DbRecord(pub Record<EncryptedData>);

macro_rules! impl_db_record_from_row {
    ($row:ty) => {
        impl<'a> ::sqlx::FromRow<'a, $row> for DbRecord {
            fn from_row(row: &'a $row) -> ::sqlx::Result<Self> {
                let idx: i64 = row.try_get("idx")?;
                let timestamp: i64 = row.try_get("timestamp")?;

                let data = EncryptedData {
                    raw: row.try_get("data")?,
                    cek: row.try_get("cek")?,
                };

                Ok(Self(Record {
                    id: row.try_get("client_id")?,
                    host: Host::new(row.try_get("host")?),
                    idx: idx as u64,
                    timestamp: timestamp as u64,
                    version: RecordVersion::from(row.try_get::<String, _>("version")?),
                    tag: RecordTag::from(row.try_get::<String, _>("tag")?),
                    data,
                }))
            }
        }
    };
}

impl_db_record_from_row!(sqlx::postgres::PgRow);
impl_db_record_from_row!(sqlx::sqlite::SqliteRow);
impl_db_record_from_row!(sqlx::mysql::MySqlRow);

#[derive(derive_more::Into)]
pub(super) struct RecordSeriesPoint {
    pub(super) series: RecordSeriesKey,
    pub(super) idx: RecordIdx,
}

macro_rules! impl_record_series_point_from_row {
    ($row:ty) => {
        impl<'a> ::sqlx::FromRow<'a, $row> for RecordSeriesPoint {
            fn from_row(row: &'a $row) -> ::sqlx::Result<Self> {
                let host: HostId = row.try_get("host")?;
                let tag: String = row.try_get("tag")?;
                let idx: i64 = row.try_get("idx")?;

                Ok(Self {
                    series: RecordSeriesKey::new(host, RecordTag::from(tag)),
                    idx: idx as u64,
                })
            }
        }
    };
}

impl_record_series_point_from_row!(sqlx::postgres::PgRow);
impl_record_series_point_from_row!(sqlx::sqlite::SqliteRow);
impl_record_series_point_from_row!(sqlx::mysql::MySqlRow);
