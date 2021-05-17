use futures::TryStreamExt;
use sqlx::{Connection, Executor, FromRow, Postgres};
use sqlx_core::postgres::types::PgRange;
use sqlx_test::{new, test_type};
use std::fmt::Debug;
use std::ops::Bound;

// Transparent types are rust-side wrappers over DB types
#[derive(PartialEq, Debug, sqlx::Type)]
#[sqlx(transparent)]
struct Transparent(i32);

// "Weak" enums map to an integer type indicated by #[repr]
#[derive(PartialEq, Copy, Clone, Debug, sqlx::Type)]
#[repr(i32)]
enum Weak {
    One = 0,
    Two = 2,
    Three = 4,
}

// "Strong" enums can map to TEXT (25)
#[derive(PartialEq, Debug, sqlx::Type)]
#[sqlx(type_name = "text")]
#[sqlx(rename_all = "lowercase")]
enum Strong {
    One,
    Two,

    #[sqlx(rename = "four")]
    Three,
}

// rename_all variants
#[derive(PartialEq, Debug, sqlx::Type)]
#[sqlx(type_name = "color_lower")]
#[sqlx(rename_all = "lowercase")]
enum ColorLower {
    Red,
    Green,
    Blue,
}

#[derive(PartialEq, Debug, sqlx::Type)]
#[sqlx(type_name = "color_snake")]
#[sqlx(rename_all = "snake_case")]
enum ColorSnake {
    RedGreen,
    BlueBlack,
}

#[derive(PartialEq, Debug, sqlx::Type)]
#[sqlx(type_name = "color_upper")]
#[sqlx(rename_all = "UPPERCASE")]
enum ColorUpper {
    Red,
    Green,
    Blue,
}

#[derive(PartialEq, Debug, sqlx::Type)]
#[sqlx(type_name = "color_screaming_snake")]
#[sqlx(rename_all = "SCREAMING_SNAKE_CASE")]
enum ColorScreamingSnake {
    RedGreen,
    BlueBlack,
}

#[derive(PartialEq, Debug, sqlx::Type)]
#[sqlx(type_name = "color_kebab_case")]
#[sqlx(rename_all = "kebab-case")]
enum ColorKebabCase {
    RedGreen,
    BlueBlack,
}

#[derive(PartialEq, Debug, sqlx::Type)]
#[sqlx(type_name = "color_mixed_case")]
#[sqlx(rename_all = "camelCase")]
enum ColorCamelCase {
    RedGreen,
    BlueBlack,
}

#[derive(PartialEq, Debug, sqlx::Type)]
#[sqlx(type_name = "color_camel_case")]
#[sqlx(rename_all = "PascalCase")]
enum ColorPascalCase {
    RedGreen,
    BlueBlack,
}

// "Strong" enum can map to a custom type
#[derive(PartialEq, Debug, sqlx::Type)]
#[sqlx(type_name = "mood")]
#[sqlx(rename_all = "lowercase")]
enum Mood {
    Ok,
    Happy,
    Sad,
}

// Records must map to a custom type
// Note that all types are types in Postgres
#[derive(PartialEq, Debug, sqlx::Type)]
#[sqlx(type_name = "inventory_item")]
struct InventoryItem {
    name: String,
    supplier_id: Option<i32>,
    price: Option<i64>,
}

// Custom range type
#[derive(sqlx::Type, Debug, PartialEq)]
#[sqlx(type_name = "float_range")]
struct FloatRange(PgRange<f64>);

// Custom domain type
#[derive(sqlx::Type, Debug)]
#[sqlx(type_name = "int4rangeL0pC")]
struct RangeInclusive(PgRange<i32>);

test_type!(transparent<Transparent>(Postgres,
    "0" == Transparent(0),
    "23523" == Transparent(23523)
));

test_type!(weak_enum<Weak>(Postgres,
    "0::int4" == Weak::One,
    "2::int4" == Weak::Two,
    "4::int4" == Weak::Three
));

test_type!(strong_enum<Strong>(Postgres,
    "'one'::text" == Strong::One,
    "'two'::text" == Strong::Two,
    "'four'::text" == Strong::Three
));

test_type!(floatrange<FloatRange>(Postgres,
    "'[1.234, 5.678]'::float_range" == FloatRange(PgRange::from((Bound::Included(1.234), Bound::Included(5.678)))),
));

#[sqlx_macros::test]
async fn test_enum_type() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    conn.execute(
        r#"
DROP TABLE IF EXISTS people;

DROP TYPE IF EXISTS mood CASCADE;

CREATE TYPE mood AS ENUM ( 'ok', 'happy', 'sad' );

DROP TYPE IF EXISTS color_lower CASCADE;
DROP TYPE IF EXISTS color_snake CASCADE;
DROP TYPE IF EXISTS color_upper CASCADE;
DROP TYPE IF EXISTS color_screaming_snake CASCADE;
DROP TYPE IF EXISTS color_kebab_case CASCADE;
DROP TYPE IF EXISTS color_mixed_case CASCADE;
DROP TYPE IF EXISTS color_camel_case CASCADE;


CREATE TYPE color_lower AS ENUM ( 'red', 'green', 'blue' );
CREATE TYPE color_snake AS ENUM ( 'red_green', 'blue_black' );
CREATE TYPE color_upper AS ENUM ( 'RED', 'GREEN', 'BLUE' );
CREATE TYPE color_screaming_snake AS ENUM ( 'RED_GREEN', 'BLUE_BLACK' );
CREATE TYPE color_kebab_case AS ENUM ( 'red-green', 'blue-black' );
CREATE TYPE color_mixed_case AS ENUM ( 'redGreen', 'blueBlack' );
CREATE TYPE color_camel_case AS ENUM ( 'RedGreen', 'BlueBlack' );


CREATE TABLE people (
    id      serial PRIMARY KEY,
    mood    mood not null
);
    "#,
    )
    .await?;

    // Drop and re-acquire the connection
    conn.close().await?;
    let mut conn = new::<Postgres>().await?;

    // Select from table test
    let (people_id,): (i32,) = sqlx::query_as(
        "
INSERT INTO people (mood)
VALUES ($1)
RETURNING id
        ",
    )
    .bind(Mood::Sad)
    .fetch_one(&mut conn)
    .await?;

    // Drop and re-acquire the connection
    conn.close().await?;
    let mut conn = new::<Postgres>().await?;

    #[derive(sqlx::FromRow)]
    struct PeopleRow {
        id: i32,
        mood: Mood,
    }

    let rec: PeopleRow = sqlx::query_as(
        "
SELECT id, mood FROM people WHERE id = $1
            ",
    )
    .bind(people_id)
    .fetch_one(&mut conn)
    .await?;

    assert_eq!(rec.id, people_id);
    assert_eq!(rec.mood, Mood::Sad);

    // Drop and re-acquire the connection
    conn.close().await?;
    let mut conn = new::<Postgres>().await?;

    let stmt = format!("SELECT id, mood FROM people WHERE id = {}", people_id);
    dbg!(&stmt);

    let mut cursor = conn.fetch(&*stmt);

    let row = cursor.try_next().await?.unwrap();
    let rec = PeopleRow::from_row(&row)?;

    assert_eq!(rec.id, people_id);
    assert_eq!(rec.mood, Mood::Sad);

    drop(cursor);

    // Normal type equivalency test

    let rec: (bool, Mood) = sqlx::query_as(
        "
    SELECT $1 = 'happy'::mood, $1
            ",
    )
    .bind(&Mood::Happy)
    .fetch_one(&mut conn)
    .await?;

    assert!(rec.0);
    assert_eq!(rec.1, Mood::Happy);

    let rec: (bool, ColorLower) = sqlx::query_as(
        "
    SELECT $1 = 'green'::color_lower, $1
            ",
    )
    .bind(&ColorLower::Green)
    .fetch_one(&mut conn)
    .await?;

    assert!(rec.0);
    assert_eq!(rec.1, ColorLower::Green);

    let rec: (bool, ColorSnake) = sqlx::query_as(
        "
    SELECT $1 = 'red_green'::color_snake, $1
            ",
    )
    .bind(&ColorSnake::RedGreen)
    .fetch_one(&mut conn)
    .await?;

    assert!(rec.0);
    assert_eq!(rec.1, ColorSnake::RedGreen);

    let rec: (bool, ColorUpper) = sqlx::query_as(
        "
    SELECT $1 = 'RED'::color_upper, $1
            ",
    )
    .bind(&ColorUpper::Red)
    .fetch_one(&mut conn)
    .await?;

    assert!(rec.0);
    assert_eq!(rec.1, ColorUpper::Red);

    let rec: (bool, ColorScreamingSnake) = sqlx::query_as(
        "
    SELECT $1 = 'RED_GREEN'::color_screaming_snake, $1
            ",
    )
    .bind(&ColorScreamingSnake::RedGreen)
    .fetch_one(&mut conn)
    .await?;

    assert!(rec.0);
    assert_eq!(rec.1, ColorScreamingSnake::RedGreen);

    let rec: (bool, ColorKebabCase) = sqlx::query_as(
        "
    SELECT $1 = 'red-green'::color_kebab_case, $1
            ",
    )
    .bind(&ColorKebabCase::RedGreen)
    .fetch_one(&mut conn)
    .await?;

    assert!(rec.0);
    assert_eq!(rec.1, ColorKebabCase::RedGreen);

    let rec: (bool, ColorCamelCase) = sqlx::query_as(
        "
    SELECT $1 = 'redGreen'::color_mixed_case, $1
            ",
    )
    .bind(&ColorCamelCase::RedGreen)
    .fetch_one(&mut conn)
    .await?;

    assert!(rec.0);
    assert_eq!(rec.1, ColorCamelCase::RedGreen);

    let rec: (bool, ColorPascalCase) = sqlx::query_as(
        "
    SELECT $1 = 'RedGreen'::color_camel_case, $1
            ",
    )
    .bind(&ColorPascalCase::RedGreen)
    .fetch_one(&mut conn)
    .await?;

    assert!(rec.0);
    assert_eq!(rec.1, ColorPascalCase::RedGreen);

    Ok(())
}

#[sqlx_macros::test]
async fn test_record_type() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    let value = InventoryItem {
        name: "fuzzy dice".to_owned(),
        supplier_id: Some(42),
        price: Some(199),
    };

    let rec: (bool, InventoryItem) = sqlx::query_as(
        "
SELECT $1 = ROW('fuzzy dice', 42, 199)::inventory_item, $1
        ",
    )
    .bind(&value)
    .fetch_one(&mut conn)
    .await?;

    assert!(rec.0);
    assert_eq!(rec.1, value);

    Ok(())
}

#[cfg(feature = "macros")]
#[sqlx_macros::test]
async fn test_from_row() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    #[derive(sqlx::FromRow)]
    struct Account {
        id: i32,
        name: String,
    }

    let account: Account = sqlx::query_as(
        "SELECT * from (VALUES (1, 'Herp Derpinson')) accounts(id, name) where id = $1",
    )
    .bind(1_i32)
    .fetch_one(&mut conn)
    .await?;

    assert_eq!(account.id, 1);
    assert_eq!(account.name, "Herp Derpinson");

    // A _single_ lifetime may be used but only when using the lowest-level API currently (Query::fetch)

    #[derive(sqlx::FromRow)]
    struct RefAccount<'a> {
        id: i32,
        name: &'a str,
    }

    let mut cursor = sqlx::query(
        "SELECT * from (VALUES (1, 'Herp Derpinson')) accounts(id, name) where id = $1",
    )
    .bind(1_i32)
    .fetch(&mut conn);

    let row = cursor.try_next().await?.unwrap();
    let account = RefAccount::from_row(&row)?;

    assert_eq!(account.id, 1);
    assert_eq!(account.name, "Herp Derpinson");

    Ok(())
}

#[cfg(feature = "macros")]
#[sqlx_macros::test]
async fn test_from_row_with_keyword() -> anyhow::Result<()> {
    #[derive(Debug, sqlx::FromRow)]
    struct AccountKeyword {
        r#type: i32,
        r#static: String,
        r#let: Option<String>,
        r#struct: Option<String>,
        name: Option<String>,
    }

    let mut conn = new::<Postgres>().await?;

    let account: AccountKeyword = sqlx::query_as(
        r#"SELECT * from (VALUES (1, 'foo', 'bar', null, null)) accounts(type, static, let, struct, name)"#
    )
    .fetch_one(&mut conn)
    .await?;
    println!("{:?}", account);

    assert_eq!(1, account.r#type);
    assert_eq!("foo", account.r#static);
    assert_eq!(None, account.r#struct);
    assert_eq!(Some("bar".to_owned()), account.r#let);
    assert_eq!(None, account.name);

    Ok(())
}

#[cfg(feature = "macros")]
#[sqlx_macros::test]
async fn test_from_row_with_rename() -> anyhow::Result<()> {
    #[derive(Debug, sqlx::FromRow)]
    struct AccountKeyword {
        #[sqlx(rename = "type")]
        own_type: i32,

        #[sqlx(rename = "static")]
        my_static: String,

        #[sqlx(rename = "let")]
        custom_let: Option<String>,

        #[sqlx(rename = "struct")]
        def_struct: Option<String>,

        name: Option<String>,
    }

    let mut conn = new::<Postgres>().await?;

    let account: AccountKeyword = sqlx::query_as(
        r#"SELECT * from (VALUES (1, 'foo', 'bar', null, null)) accounts(type, static, let, struct, name)"#
    )
    .fetch_one(&mut conn)
    .await?;
    println!("{:?}", account);

    assert_eq!(1, account.own_type);
    assert_eq!("foo", account.my_static);
    assert_eq!(None, account.def_struct);
    assert_eq!(Some("bar".to_owned()), account.custom_let);
    assert_eq!(None, account.name);

    Ok(())
}

#[cfg(feature = "macros")]
#[sqlx_macros::test]
async fn test_from_row_with_rename_all() -> anyhow::Result<()> {
    #[derive(Debug, sqlx::FromRow)]
    #[sqlx(rename_all = "camelCase")]
    struct AccountKeyword {
        user_id: i32,
        user_name: String,
        user_surname: String,
    }

    let mut conn = new::<Postgres>().await?;

    let account: AccountKeyword = sqlx::query_as(
        r#"SELECT * from (VALUES (1, 'foo', 'bar')) accounts("userId", "userName", "userSurname")"#,
    )
    .fetch_one(&mut conn)
    .await?;
    println!("{:?}", account);

    assert_eq!(1, account.user_id);
    assert_eq!("foo", account.user_name);
    assert_eq!("bar", account.user_surname);

    Ok(())
}

#[cfg(feature = "macros")]
#[sqlx_macros::test]
async fn test_from_row_tuple() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    #[derive(Debug, sqlx::FromRow)]
    struct Account(i32, String);

    let account: Account = sqlx::query_as(
        "SELECT * from (VALUES (1, 'Herp Derpinson')) accounts(id, name) where id = $1",
    )
    .bind(1_i32)
    .fetch_one(&mut conn)
    .await?;

    assert_eq!(account.0, 1);
    assert_eq!(account.1, "Herp Derpinson");

    // A _single_ lifetime may be used but only when using the lowest-level API currently (Query::fetch)

    #[derive(sqlx::FromRow)]
    struct RefAccount<'a>(i32, &'a str);

    let mut cursor = sqlx::query(
        "SELECT * from (VALUES (1, 'Herp Derpinson')) accounts(id, name) where id = $1",
    )
    .bind(1_i32)
    .fetch(&mut conn);

    let row = cursor.try_next().await?.unwrap();
    let account = RefAccount::from_row(&row)?;

    assert_eq!(account.0, 1);
    assert_eq!(account.1, "Herp Derpinson");

    Ok(())
}

#[cfg(feature = "macros")]
#[sqlx_macros::test]
async fn test_default() -> anyhow::Result<()> {
    #[derive(Debug, sqlx::FromRow)]
    struct HasDefault {
        not_default: i32,
        #[sqlx(default)]
        default: Option<i32>,
    }

    let mut conn = new::<Postgres>().await?;

    let has_default: HasDefault = sqlx::query_as(r#"SELECT 1 AS not_default"#)
        .fetch_one(&mut conn)
        .await?;
    println!("{:?}", has_default);

    assert_eq!(has_default.not_default, 1);
    assert_eq!(has_default.default, None);

    Ok(())
}
