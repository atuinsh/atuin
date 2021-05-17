extern crate time_ as time;

#[cfg(feature = "decimal")]
use std::str::FromStr;

use sqlx::mysql::MySql;
use sqlx::{Executor, Row};
use sqlx_test::{new, test_type};

test_type!(bool(MySql, "false" == false, "true" == true));

test_type!(u8(MySql, "CAST(253 AS UNSIGNED)" == 253_u8));
test_type!(i8(MySql, "5" == 5_i8, "0" == 0_i8));

test_type!(u16(MySql, "CAST(21415 AS UNSIGNED)" == 21415_u16));
test_type!(i16(MySql, "21415" == 21415_i16));

test_type!(u32(MySql, "CAST(2141512 AS UNSIGNED)" == 2141512_u32));
test_type!(i32(MySql, "2141512" == 2141512_i32));

test_type!(u64(MySql, "CAST(2141512 AS UNSIGNED)" == 2141512_u64));
test_type!(i64(MySql, "2141512" == 2141512_i64));

test_type!(f64(MySql, "3.14159265e0" == 3.14159265_f64));

// NOTE: This behavior can be very surprising. MySQL implicitly widens FLOAT bind parameters
//       to DOUBLE. This results in the weirdness you see below. MySQL generally recommends to stay
//       away from FLOATs.
test_type!(f32(MySql, "3.1410000324249268e0" == 3.141f32 as f64 as f32));

test_type!(string<String>(MySql,
    "'helloworld'" == "helloworld",
    "''" == ""
));

test_type!(bytes<Vec<u8>>(MySql,
    "X'DEADBEEF'"
        == vec![0xDE_u8, 0xAD, 0xBE, 0xEF],
    "X''"
        == Vec::<u8>::new(),
    "X'0000000052'"
        == vec![0_u8, 0, 0, 0, 0x52]
));

#[cfg(feature = "uuid")]
test_type!(uuid<sqlx::types::Uuid>(MySql,
    "x'b731678f636f4135bc6f19440c13bd19'"
        == sqlx::types::Uuid::parse_str("b731678f-636f-4135-bc6f-19440c13bd19").unwrap(),
    "x'00000000000000000000000000000000'"
        == sqlx::types::Uuid::parse_str("00000000-0000-0000-0000-000000000000").unwrap()
));

#[cfg(feature = "uuid")]
test_type!(uuid_hyphenated<sqlx::types::uuid::adapter::Hyphenated>(MySql,
    "'b731678f-636f-4135-bc6f-19440c13bd19'"
        == sqlx::types::Uuid::parse_str("b731678f-636f-4135-bc6f-19440c13bd19").unwrap().to_hyphenated(),
    "'00000000-0000-0000-0000-000000000000'"
        == sqlx::types::Uuid::parse_str("00000000-0000-0000-0000-000000000000").unwrap().to_hyphenated()
));

#[cfg(feature = "chrono")]
mod chrono {
    use super::*;
    use sqlx::types::chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};

    test_type!(chrono_date<NaiveDate>(MySql,
        "DATE '2001-01-05'" == NaiveDate::from_ymd(2001, 1, 5),
        "DATE '2050-11-23'" == NaiveDate::from_ymd(2050, 11, 23)
    ));

    test_type!(chrono_time_zero<NaiveTime>(MySql,
        "TIME '00:00:00.000000'" == NaiveTime::from_hms_micro(0, 0, 0, 0)
    ));

    test_type!(chrono_time<NaiveTime>(MySql,
        "TIME '05:10:20.115100'" == NaiveTime::from_hms_micro(5, 10, 20, 115100)
    ));

    test_type!(chrono_date_time<NaiveDateTime>(MySql,
        "TIMESTAMP '2019-01-02 05:10:20'" == NaiveDate::from_ymd(2019, 1, 2).and_hms(5, 10, 20)
    ));

    test_type!(chrono_timestamp<DateTime::<Utc>>(MySql,
        "TIMESTAMP '2019-01-02 05:10:20.115100'"
            == DateTime::<Utc>::from_utc(
                NaiveDate::from_ymd(2019, 1, 2).and_hms_micro(5, 10, 20, 115100),
                Utc,
            )
    ));

    #[sqlx_macros::test]
    async fn test_type_chrono_zero_date() -> anyhow::Result<()> {
        let mut conn = sqlx_test::new::<MySql>().await?;

        // ensure that zero dates are turned on
        // newer MySQL has these disabled by default

        conn.execute("SET @@sql_mode := REPLACE(@@sql_mode, 'NO_ZERO_IN_DATE', '');")
            .await?;

        conn.execute("SET @@sql_mode := REPLACE(@@sql_mode, 'NO_ZERO_DATE', '');")
            .await?;

        // date

        let row = sqlx::query("SELECT DATE '0000-00-00'")
            .fetch_one(&mut conn)
            .await?;

        let val: Option<NaiveDate> = row.get(0);

        assert_eq!(val, None);
        assert!(row.try_get::<NaiveDate, _>(0).is_err());

        // datetime

        let row = sqlx::query("SELECT TIMESTAMP '0000-00-00 00:00:00'")
            .fetch_one(&mut conn)
            .await?;

        let val: Option<NaiveDateTime> = row.get(0);

        assert_eq!(val, None);
        assert!(row.try_get::<NaiveDateTime, _>(0).is_err());

        Ok(())
    }
}

#[cfg(feature = "time")]
mod time_tests {
    use super::*;
    use sqlx::types::time::{Date, OffsetDateTime, PrimitiveDateTime, Time};
    use time::{date, time};

    test_type!(time_date<Date>(
        MySql,
        "DATE '2001-01-05'" == date!(2001 - 1 - 5),
        "DATE '2050-11-23'" == date!(2050 - 11 - 23)
    ));

    test_type!(time_time_zero<Time>(
        MySql,
        "TIME '00:00:00.000000'" == time!(00:00:00.000000)
    ));

    test_type!(time_time<Time>(
        MySql,
        "TIME '05:10:20.115100'" == time!(5:10:20.115100)
    ));

    test_type!(time_date_time<PrimitiveDateTime>(
        MySql,
        "TIMESTAMP '2019-01-02 05:10:20'" == date!(2019 - 1 - 2).with_time(time!(5:10:20)),
        "TIMESTAMP '2019-01-02 05:10:20.115100'"
            == date!(2019 - 1 - 2).with_time(time!(5:10:20.115100))
    ));

    test_type!(time_timestamp<OffsetDateTime>(
        MySql,
        "TIMESTAMP '2019-01-02 05:10:20.115100'"
            == date!(2019 - 1 - 2)
                .with_time(time!(5:10:20.115100))
                .assume_utc()
    ));

    #[sqlx_macros::test]
    async fn test_type_time_zero_date() -> anyhow::Result<()> {
        let mut conn = sqlx_test::new::<MySql>().await?;

        // ensure that zero dates are turned on
        // newer MySQL has these disabled by default

        conn.execute("SET @@sql_mode := REPLACE(@@sql_mode, 'NO_ZERO_IN_DATE', '');")
            .await?;

        conn.execute("SET @@sql_mode := REPLACE(@@sql_mode, 'NO_ZERO_DATE', '');")
            .await?;

        // date

        let row = sqlx::query("SELECT DATE '0000-00-00'")
            .fetch_one(&mut conn)
            .await?;

        let val: Option<Date> = row.get(0);

        assert_eq!(val, None);
        assert!(row.try_get::<Date, _>(0).is_err());

        // datetime

        let row = sqlx::query("SELECT TIMESTAMP '0000-00-00 00:00:00'")
            .fetch_one(&mut conn)
            .await?;

        let val: Option<PrimitiveDateTime> = row.get(0);

        assert_eq!(val, None);
        assert!(row.try_get::<PrimitiveDateTime, _>(0).is_err());

        Ok(())
    }
}

#[cfg(feature = "bigdecimal")]
test_type!(bigdecimal<sqlx::types::BigDecimal>(
    MySql,
    "CAST(0 as DECIMAL(0, 0))" == "0".parse::<sqlx::types::BigDecimal>().unwrap(),
    "CAST(1 AS DECIMAL(1, 0))" == "1".parse::<sqlx::types::BigDecimal>().unwrap(),
    "CAST(10000 AS DECIMAL(5, 0))" == "10000".parse::<sqlx::types::BigDecimal>().unwrap(),
    "CAST(0.1 AS DECIMAL(2, 1))" == "0.1".parse::<sqlx::types::BigDecimal>().unwrap(),
    "CAST(0.01234 AS DECIMAL(6, 5))" == "0.01234".parse::<sqlx::types::BigDecimal>().unwrap(),
    "CAST(12.34 AS DECIMAL(4, 2))" == "12.34".parse::<sqlx::types::BigDecimal>().unwrap(),
    "CAST(12345.6789 AS DECIMAL(9, 4))" == "12345.6789".parse::<sqlx::types::BigDecimal>().unwrap(),
));

#[cfg(feature = "decimal")]
test_type!(decimal<sqlx::types::Decimal>(MySql,
    "CAST(0 as DECIMAL(0, 0))" == sqlx::types::Decimal::from_str("0").unwrap(),
    "CAST(1 AS DECIMAL(1, 0))" == sqlx::types::Decimal::from_str("1").unwrap(),
    "CAST(10000 AS DECIMAL(5, 0))" == sqlx::types::Decimal::from_str("10000").unwrap(),
    "CAST(0.1 AS DECIMAL(2, 1))" == sqlx::types::Decimal::from_str("0.1").unwrap(),
    "CAST(0.01234 AS DECIMAL(6, 5))" == sqlx::types::Decimal::from_str("0.01234").unwrap(),
    "CAST(12.34 AS DECIMAL(4, 2))" == sqlx::types::Decimal::from_str("12.34").unwrap(),
    "CAST(12345.6789 AS DECIMAL(9, 4))" == sqlx::types::Decimal::from_str("12345.6789").unwrap(),
));

#[cfg(feature = "json")]
mod json_tests {
    use super::*;
    use serde_json::{json, Value as JsonValue};
    use sqlx::types::Json;
    use sqlx_test::test_type;

    test_type!(json<JsonValue>(
        MySql,
        "SELECT CAST({0} AS BINARY) <=> CAST(? AS BINARY), CAST({0} AS BINARY) as _2, ? as _3",
        "'\"Hello, World\"'" == json!("Hello, World"),
        "'\"😎\"'" == json!("😎"),
        "'\"🙋‍♀️\"'" == json!("🙋‍♀️"),
        "'[\"Hello\",\"World!\"]'" == json!(["Hello", "World!"])
    ));

    #[derive(serde::Deserialize, serde::Serialize, Debug, PartialEq)]
    struct Friend {
        name: String,
        age: u32,
    }

    test_type!(json_struct<Json<Friend>>(
        MySql,
        "SELECT CAST({0} AS BINARY) <=> CAST(? AS BINARY), CAST({0} AS BINARY) as _2, ? as _3",
        "\'{\"name\":\"Joe\",\"age\":33}\'" == Json(Friend { name: "Joe".to_string(), age: 33 })
    ));

    // NOTE: This is testing recursive (and transparent) usage of the `Json` wrapper. You don't
    //       need to wrap the Vec in Json<_> to make the example work.

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Customer {
        json_column: Json<Vec<i64>>,
    }

    test_type!(json_struct_json_column<Json<Customer>>(
        MySql,
        "\'{\"json_column\":[1,2]}\'" == Json(Customer { json_column: Json(vec![1, 2]) })
    ));
}

#[sqlx_macros::test]
async fn test_bits() -> anyhow::Result<()> {
    let mut conn = new::<MySql>().await?;

    conn.execute(
        r#"
CREATE TEMPORARY TABLE with_bits (
    id INT PRIMARY KEY AUTO_INCREMENT,
    value_1 BIT(1) NOT NULL,
    value_n BIT(64) NOT NULL
);
    "#,
    )
    .await?;

    sqlx::query("INSERT INTO with_bits (value_1, value_n) VALUES (?, ?)")
        .bind(&1_u8)
        .bind(&510202_u32)
        .execute(&mut conn)
        .await?;

    // BINARY
    let (v1, vn): (u8, u64) = sqlx::query_as("SELECT value_1, value_n FROM with_bits")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(v1, 1);
    assert_eq!(vn, 510202);

    // TEXT
    let row = conn
        .fetch_one("SELECT value_1, value_n FROM with_bits")
        .await?;
    let v1: u8 = row.try_get(0)?;
    let vn: u64 = row.try_get(1)?;

    assert_eq!(v1, 1);
    assert_eq!(vn, 510202);

    Ok(())
}
