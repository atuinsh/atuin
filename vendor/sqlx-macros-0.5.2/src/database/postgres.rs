use sqlx_core as sqlx;

impl_database_ext! {
    sqlx::postgres::Postgres {
        (),
        bool,
        String | &str,
        i8,
        i16,
        i32,
        u32,
        i64,
        f32,
        f64,
        Vec<u8> | &[u8],

        sqlx::postgres::types::PgInterval,

        #[cfg(feature = "uuid")]
        sqlx::types::Uuid,

        #[cfg(feature = "chrono")]
        sqlx::types::chrono::NaiveTime,

        #[cfg(feature = "chrono")]
        sqlx::types::chrono::NaiveDate,

        #[cfg(feature = "chrono")]
        sqlx::types::chrono::NaiveDateTime,

        #[cfg(feature = "chrono")]
        sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc> | sqlx::types::chrono::DateTime<_>,

        #[cfg(feature = "chrono")]
        sqlx::postgres::types::PgTimeTz<sqlx::types::chrono::NaiveTime, sqlx::types::chrono::FixedOffset>,

        #[cfg(feature = "time")]
        sqlx::types::time::Time,

        #[cfg(feature = "time")]
        sqlx::types::time::Date,

        #[cfg(feature = "time")]
        sqlx::types::time::PrimitiveDateTime,

        #[cfg(feature = "time")]
        sqlx::types::time::OffsetDateTime,

        #[cfg(feature = "time")]
        sqlx::postgres::types::PgTimeTz<sqlx::types::time::Time, sqlx::types::time::UtcOffset>,

        #[cfg(feature = "bigdecimal")]
        sqlx::types::BigDecimal,

        #[cfg(feature = "decimal")]
        sqlx::types::Decimal,

        #[cfg(feature = "ipnetwork")]
        sqlx::types::ipnetwork::IpNetwork,

        #[cfg(feature = "json")]
        serde_json::Value,

        #[cfg(feature = "bit-vec")]
        sqlx::types::BitVec,

        // Arrays

        Vec<bool> | &[bool],
        Vec<String> | &[String],
        Vec<Vec<u8>> | &[Vec<u8>],
        Vec<i8> | &[i8],
        Vec<i16> | &[i16],
        Vec<i32> | &[i32],
        Vec<u32> | &[u32],
        Vec<i64> | &[i64],
        Vec<f32> | &[f32],
        Vec<f64> | &[f64],

        #[cfg(feature = "uuid")]
        Vec<sqlx::types::Uuid> | &[sqlx::types::Uuid],

        #[cfg(feature = "chrono")]
        Vec<sqlx::types::chrono::NaiveTime> | &[sqlx::types::chrono::NaiveTime],

        #[cfg(feature = "chrono")]
        Vec<sqlx::types::chrono::NaiveDate> | &[sqlx::types::chrono::NaiveDate],

        #[cfg(feature = "chrono")]
        Vec<sqlx::types::chrono::NaiveDateTime> | &[sqlx::types::chrono::NaiveDateTime],

        #[cfg(feature = "chrono")]
        Vec<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>> | &[sqlx::types::chrono::DateTime<_>],

        #[cfg(feature = "time")]
        Vec<sqlx::types::time::Time> | &[sqlx::types::time::Time],

        #[cfg(feature = "time")]
        Vec<sqlx::types::time::Date> | &[sqlx::types::time::Date],

        #[cfg(feature = "time")]
        Vec<sqlx::types::time::PrimitiveDateTime> | &[sqlx::types::time::PrimitiveDateTime],

        #[cfg(feature = "time")]
        Vec<sqlx::types::time::OffsetDateTime> | &[sqlx::types::time::OffsetDateTime],

        #[cfg(feature = "bigdecimal")]
        Vec<sqlx::types::BigDecimal> | &[sqlx::types::BigDecimal],

        #[cfg(feature = "ipnetwork")]
        Vec<sqlx::types::ipnetwork::IpNetwork> | &[sqlx::types::ipnetwork::IpNetwork],

        #[cfg(feature = "json")]
        Vec<serde_json::Value> | &[serde_json::Value],

        // Ranges

        sqlx::postgres::types::PgRange<i32>,
        sqlx::postgres::types::PgRange<i64>,

        #[cfg(feature = "bigdecimal")]
        sqlx::postgres::types::PgRange<sqlx::types::BigDecimal>,

        #[cfg(feature = "chrono")]
        sqlx::postgres::types::PgRange<sqlx::types::chrono::NaiveDate>,

        #[cfg(feature = "chrono")]
        sqlx::postgres::types::PgRange<sqlx::types::chrono::NaiveDateTime>,

        #[cfg(feature = "chrono")]
        sqlx::postgres::types::PgRange<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>> |
            sqlx::postgres::types::PgRange<sqlx::types::chrono::DateTime<_>>,

        #[cfg(feature = "time")]
        sqlx::postgres::types::PgRange<sqlx::types::time::Date>,

        #[cfg(feature = "time")]
        sqlx::postgres::types::PgRange<sqlx::types::time::PrimitiveDateTime>,

        #[cfg(feature = "time")]
        sqlx::postgres::types::PgRange<sqlx::types::time::OffsetDateTime>,

        // Range arrays

        Vec<sqlx::postgres::types::PgRange<i32>> | &[sqlx::postgres::types::PgRange<i32>],
        Vec<sqlx::postgres::types::PgRange<i64>> | &[sqlx::postgres::types::PgRange<i64>],

        #[cfg(feature = "bigdecimal")]
        Vec<sqlx::postgres::types::PgRange<sqlx::types::BigDecimal>> |
            &[sqlx::postgres::types::PgRange<sqlx::types::BigDecimal>],

        #[cfg(feature = "chrono")]
        Vec<sqlx::postgres::types::PgRange<sqlx::types::chrono::NaiveDate>> |
            &[sqlx::postgres::types::PgRange<sqlx::types::chrono::NaiveDate>],

        #[cfg(feature = "chrono")]
        Vec<sqlx::postgres::types::PgRange<sqlx::types::chrono::NaiveDateTime>> |
            &[sqlx::postgres::types::PgRange<sqlx::types::chrono::NaiveDateTime>],

        #[cfg(feature = "chrono")]
        Vec<sqlx::postgres::types::PgRange<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>> |
            Vec<sqlx::postgres::types::PgRange<sqlx::types::chrono::DateTime<_>>>,

        #[cfg(feature = "chrono")]
        &[sqlx::postgres::types::PgRange<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>] |
            &[sqlx::postgres::types::PgRange<sqlx::types::chrono::DateTime<_>>],

        #[cfg(feature = "time")]
        Vec<sqlx::postgres::types::PgRange<sqlx::types::time::Date>> |
            &[sqlx::postgres::types::PgRange<sqlx::types::time::Date>],

        #[cfg(feature = "time")]
        Vec<sqlx::postgres::types::PgRange<sqlx::types::time::PrimitiveDateTime>> |
            &[sqlx::postgres::types::PgRange<sqlx::types::time::PrimitiveDateTime>],

        #[cfg(feature = "time")]
        Vec<sqlx::postgres::types::PgRange<sqlx::types::time::OffsetDateTime>> |
            &[sqlx::postgres::types::PgRange<sqlx::types::time::OffsetDateTime>],
    },
    ParamChecking::Strong,
    feature-types: info => info.__type_feature_gate(),
    row = sqlx::postgres::PgRow,
    name = "PostgreSQL"
}
