use sea_orm::entity::prelude::*;

/// The `total_history_count_user` table exists on **Postgres only** (maintained by a
/// pg trigger). `delete_user` purges it, but only on pg — the shared query layer gates
/// this delete on the backend so mysql/sqlite (which lack the table) never touch it.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "total_history_count_user")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub user_id: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
