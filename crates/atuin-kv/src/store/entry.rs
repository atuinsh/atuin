#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct KvEntry {
    pub namespace: String,
    pub key: String,
    pub value: String,
}
