use atuin_domain::record::{
    EncryptedData, Host, HostId, Record, RecordId, RecordTag, RecordVersion,
};
use sqlx::Row;
use sqlx::mysql::MySqlRow;

#[derive(derive_more::Into)]
pub struct DbRecord(pub Record<EncryptedData>);

impl<'a> ::sqlx::FromRow<'a, MySqlRow> for DbRecord {
    fn from_row(row: &'a MySqlRow) -> ::sqlx::Result<Self> {
        let timestamp: i64 = row.try_get("timestamp")?;
        let idx: i64 = row.try_get("idx")?;

        let data = EncryptedData {
            raw: row.try_get("data")?,
            cek: row.try_get("cek")?,
        };

        let client_id_bytes: Vec<u8> = row.try_get("client_id")?;
        let client_id = uuid::Uuid::from_slice(&client_id_bytes)
            .map_err(|e| ::sqlx::Error::Decode(Box::new(e)))?;

        let host_bytes: Vec<u8> = row.try_get("host")?;
        let host_uuid =
            uuid::Uuid::from_slice(&host_bytes).map_err(|e| ::sqlx::Error::Decode(Box::new(e)))?;

        Ok(Self(Record {
            id: RecordId(client_id),
            host: Host::new(HostId(host_uuid)),
            idx: idx as u64,
            timestamp: timestamp as u64,
            version: RecordVersion::from(row.try_get::<String, _>("version")?),
            tag: RecordTag::from(row.try_get::<String, _>("tag")?),
            data,
        }))
    }
}
