use lazy_static::lazy_static;
use tauri_plugin_sql::{Builder, Migration, MigrationKind};

pub fn migrations() -> Vec<Migration> {
    vec![
        Migration {
            version: 1,
            description: "create_initial_tables",
            sql: "CREATE TABLE runbooks(id string PRIMARY KEY, name TEXT, content TEXT, created bigint, updated bigint);",
            kind: MigrationKind::Up,
        }
    ]
}
