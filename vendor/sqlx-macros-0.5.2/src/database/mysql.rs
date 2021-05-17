use sqlx_core as sqlx;

impl_database_ext! {
    sqlx::mysql::MySql {
        u8,
        u16,
        u32,
        u64,
        i8,
        i16,
        i32,
        i64,
        f32,
        f64,

        // ordering is important here as otherwise we might infer strings to be binary
        // CHAR, VAR_CHAR, TEXT
        String,

        // BINARY, VAR_BINARY, BLOB
        Vec<u8>,

        #[cfg(all(feature = "chrono", not(feature = "time")))]
        sqlx::types::chrono::NaiveTime,

        #[cfg(all(feature = "chrono", not(feature = "time")))]
        sqlx::types::chrono::NaiveDate,

        #[cfg(all(feature = "chrono", not(feature = "time")))]
        sqlx::types::chrono::NaiveDateTime,

        #[cfg(all(feature = "chrono", not(feature = "time")))]
        sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>,

        #[cfg(feature = "time")]
        sqlx::types::time::Time,

        #[cfg(feature = "time")]
        sqlx::types::time::Date,

        #[cfg(feature = "time")]
        sqlx::types::time::PrimitiveDateTime,

        #[cfg(feature = "time")]
        sqlx::types::time::OffsetDateTime,

        #[cfg(feature = "bigdecimal")]
        sqlx::types::BigDecimal,

        #[cfg(feature = "decimal")]
        sqlx::types::Decimal,

        #[cfg(feature = "json")]
        serde_json::Value,
    },
    ParamChecking::Weak,
    feature-types: info => info.__type_feature_gate(),
    row = sqlx::mysql::MySqlRow,
    name = "MySQL"
}
