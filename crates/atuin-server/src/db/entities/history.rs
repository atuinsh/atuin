use sea_orm::entity::prelude::*;

/// The legacy `history` table. No live trait method reads or inserts here; the only
/// use is `delete_user`, which deletes the user's rows. So we model just the columns
/// that delete needs — sea-orm only ever references the table name and `user_id`.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "history")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub user_id: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
