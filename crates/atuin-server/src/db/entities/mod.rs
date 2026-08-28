//! sea-orm entity definitions for the server database.
//!
//! One entity per table, modelling only the columns the [`Database`](super::Database)
//! trait actually touches. Because sea-orm generates dialect-correct SQL from these
//! definitions, the same entity drives Postgres, MySQL and SQLite — which is the whole
//! reason the three hand-written backends could collapse into one shared query layer.
//!
//! Note on UUID columns (`store.id`/`client_id`/`host`): the physical type differs per
//! backend (`uuid` on pg, `VARBINARY(16)` on mysql, blob-in-a-`text`-column on sqlite),
//! but every backend round-trips through sqlx's `Uuid` codec. sea-orm's `with-uuid`
//! support goes through that exact same codec, so a single `Uuid` entity column matches
//! the on-disk representation on all three — byte-for-byte with the previous sqlx code.

pub mod history;
pub mod sessions;
pub mod store;
pub mod total_history_count_user;
pub mod users;
