use sqlx_core as sqlx;

impl_database_ext! {
    sqlx::sqlite::Sqlite {
        bool,
        i32,
        i64,
        f32,
        f64,
        String,
        Vec<u8>,

        #[cfg(feature = "chrono")]
        sqlx::types::chrono::NaiveDateTime,

        #[cfg(feature = "chrono")]
        sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc> | sqlx::types::chrono::DateTime<_>,
    },
    ParamChecking::Weak,
    feature-types: _info => None,
    row = sqlx::sqlite::SqliteRow,
    name = "SQLite"
}
