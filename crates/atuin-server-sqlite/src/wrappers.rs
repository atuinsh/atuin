use atuin_domain::record::{EncryptedData, Host, Record, RecordTag, RecordVersion};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

#[derive(derive_more::Into)]
pub struct DbRecord(pub Record<EncryptedData>);

impl<'a> ::sqlx::FromRow<'a, SqliteRow> for DbRecord {
    fn from_row(row: &'a SqliteRow) -> ::sqlx::Result<Self> {
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
