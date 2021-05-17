use sqlx::sqlite::{Sqlite, SqliteRow};
use sqlx_core::row::Row;
use sqlx_test::new;
use sqlx_test::test_type;

test_type!(null<Option<i32>>(Sqlite,
    "NULL" == None::<i32>
));

test_type!(bool(Sqlite, "FALSE" == false, "TRUE" == true));

test_type!(i32(Sqlite, "94101" == 94101_i32));

test_type!(i64(Sqlite, "9358295312" == 9358295312_i64));

// NOTE: This behavior can be surprising. Floating-point parameters are widening to double which can
//       result in strange rounding.
test_type!(f32(Sqlite, "3.1410000324249268" == 3.141f32 as f64 as f32));

test_type!(f64(Sqlite, "939399419.1225182" == 939399419.1225182_f64));

test_type!(str<String>(Sqlite,
    "'this is foo'" == "this is foo",
    "cast(x'7468697320006973206E756C2D636F6E7461696E696E67' as text)" == "this \0is nul-containing",
    "''" == ""
));

test_type!(bytes<Vec<u8>>(Sqlite,
    "X'DEADBEEF'"
        == vec![0xDE_u8, 0xAD, 0xBE, 0xEF],
    "X''"
        == Vec::<u8>::new(),
    "X'0000000052'"
        == vec![0_u8, 0, 0, 0, 0x52]
));

#[cfg(feature = "json")]
mod json_tests {
    use super::*;
    use serde_json::{json, Value as JsonValue};
    use sqlx::types::Json;
    use sqlx_test::test_type;

    test_type!(json<JsonValue>(
        Sqlite,
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
        Sqlite,
        "\'{\"name\":\"Joe\",\"age\":33}\'" == Json(Friend { name: "Joe".to_string(), age: 33 })
    ));

    // NOTE: This is testing recursive (and transparent) usage of the `Json` wrapper. You don't
    //       need to wrap the Vec in Json<_> to make the example work.

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Customer {
        json_column: Json<Vec<i64>>,
    }

    test_type!(json_struct_json_column<Json<Customer>>(
        Sqlite,
        "\'{\"json_column\":[1,2]}\'" == Json(Customer { json_column: Json(vec![1, 2]) })
    ));

    #[sqlx_macros::test]
    async fn it_json_extracts() -> anyhow::Result<()> {
        let mut conn = new::<Sqlite>().await?;

        let value = sqlx::query("select JSON_EXTRACT(JSON('{ \"number\": 42 }'), '$.number') = ?1")
            .bind(42_i32)
            .try_map(|row: SqliteRow| row.try_get::<bool, _>(0))
            .fetch_one(&mut conn)
            .await?;

        assert_eq!(true, value);

        Ok(())
    }
}

#[cfg(feature = "chrono")]
mod chrono {
    use super::*;
    use sqlx::types::chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc};

    test_type!(chrono_naive_date_time<NaiveDateTime>(Sqlite,
        "datetime('2019-01-02 05:10:20')" == NaiveDate::from_ymd(2019, 1, 2).and_hms(5, 10, 20)
    ));

    test_type!(chrono_date_time_utc<DateTime::<Utc>>(Sqlite,
        "datetime('1996-12-20T00:39:57+00:00')" == Utc.ymd(1996, 12, 20).and_hms(0, 39, 57)
    ));

    test_type!(chrono_date_time_fixed_offset<DateTime::<FixedOffset>>(Sqlite,
        "datetime('2016-11-08T03:50:23-05:00')" == FixedOffset::west(5 * 3600).ymd(2016, 11, 08).and_hms(3, 50, 23)
    ));
}

#[cfg(feature = "bstr")]
mod bstr {
    use super::*;
    use sqlx::types::bstr::BString;

    test_type!(bstring<BString>(Sqlite,
        "cast('abc123' as blob)" == BString::from(&b"abc123"[..]),
        "x'0001020304'" == BString::from(&b"\x00\x01\x02\x03\x04"[..])
    ));
}

#[cfg(feature = "git2")]
mod git2 {
    use super::*;
    use sqlx::types::git2::Oid;

    test_type!(oid<Oid>(
        Sqlite,
        "x'0000000000000000000000000000000000000000'" == Oid::zero(),
        "x'000102030405060708090a0b0c0d0e0f10111213'"
            == Oid::from_str("000102030405060708090a0b0c0d0e0f10111213").unwrap()
    ));
}

#[cfg(feature = "uuid")]
test_type!(uuid<sqlx::types::Uuid>(Sqlite,
    "x'b731678f636f4135bc6f19440c13bd19'"
        == sqlx::types::Uuid::parse_str("b731678f-636f-4135-bc6f-19440c13bd19").unwrap(),
    "x'00000000000000000000000000000000'"
        == sqlx::types::Uuid::parse_str("00000000-0000-0000-0000-000000000000").unwrap()
));

#[cfg(feature = "uuid")]
test_type!(uuid_hyphenated<sqlx::types::uuid::adapter::Hyphenated>(Sqlite,
    "'b731678f-636f-4135-bc6f-19440c13bd19'"
        == sqlx::types::Uuid::parse_str("b731678f-636f-4135-bc6f-19440c13bd19").unwrap().to_hyphenated(),
    "'00000000-0000-0000-0000-000000000000'"
        == sqlx::types::Uuid::parse_str("00000000-0000-0000-0000-000000000000").unwrap().to_hyphenated()
));
