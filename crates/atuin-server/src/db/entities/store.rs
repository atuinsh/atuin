use sea_orm::entity::prelude::*;

/// The `store` table — the encrypted record WAL. `id`/`client_id`/`host` are UUIDs
/// (physically `uuid`/`VARBINARY(16)`/blob depending on backend; all via sqlx's Uuid
/// codec). The unique index `record_uniq(user_id, host, tag, idx)` is what `add_records`
/// upserts against and what `status` groups by.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "store")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub client_id: Uuid,
    pub host: Uuid,
    pub idx: i64,
    pub timestamp: i64,
    pub version: String,
    pub tag: String,
    pub data: String,
    pub cek: String,
    pub user_id: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
