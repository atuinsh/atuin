use sqlx::error::DatabaseError;
use sqlx::sqlite::{SqliteConnectOptions, SqliteError};
use sqlx::ConnectOptions;
use sqlx::TypeInfo;
use sqlx::{sqlite::Sqlite, Column, Executor};
use sqlx_test::new;
use std::env;

#[sqlx_macros::test]
async fn it_describes_simple() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    let info = conn.describe("SELECT * FROM tweet").await?;
    let columns = info.columns();

    assert_eq!(columns[0].name(), "id");
    assert_eq!(columns[1].name(), "text");
    assert_eq!(columns[2].name(), "is_sent");
    assert_eq!(columns[3].name(), "owner_id");

    assert_eq!(columns[0].ordinal(), 0);
    assert_eq!(columns[1].ordinal(), 1);
    assert_eq!(columns[2].ordinal(), 2);
    assert_eq!(columns[3].ordinal(), 3);

    assert_eq!(info.nullable(0), Some(false));
    assert_eq!(info.nullable(1), Some(false));
    assert_eq!(info.nullable(2), Some(false));
    assert_eq!(info.nullable(3), Some(true)); // owner_id

    assert_eq!(columns[0].type_info().name(), "INTEGER");
    assert_eq!(columns[1].type_info().name(), "TEXT");
    assert_eq!(columns[2].type_info().name(), "BOOLEAN");
    assert_eq!(columns[3].type_info().name(), "INTEGER");

    Ok(())
}

#[sqlx_macros::test]
async fn it_describes_variables() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    // without any context, we resolve to NULL
    let info = conn.describe("SELECT ?1").await?;

    assert_eq!(info.columns()[0].type_info().name(), "NULL");
    assert_eq!(info.nullable(0), None); // unknown

    // context can be provided by using CAST(_ as _)
    let info = conn.describe("SELECT CAST(?1 AS REAL)").await?;

    assert_eq!(info.columns()[0].type_info().name(), "REAL");
    assert_eq!(info.nullable(0), None); // unknown

    Ok(())
}

#[sqlx_macros::test]
async fn it_describes_expression() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    let d = conn
        .describe("SELECT 1 + 10, 5.12 * 2, 'Hello', x'deadbeef'")
        .await?;

    let columns = d.columns();

    assert_eq!(columns[0].type_info().name(), "INTEGER");
    assert_eq!(columns[0].name(), "1 + 10");
    assert_eq!(d.nullable(0), Some(false)); // literal constant

    assert_eq!(columns[1].type_info().name(), "REAL");
    assert_eq!(columns[1].name(), "5.12 * 2");
    assert_eq!(d.nullable(1), Some(false)); // literal constant

    assert_eq!(columns[2].type_info().name(), "TEXT");
    assert_eq!(columns[2].name(), "'Hello'");
    assert_eq!(d.nullable(2), Some(false)); // literal constant

    assert_eq!(columns[3].type_info().name(), "BLOB");
    assert_eq!(columns[3].name(), "x'deadbeef'");
    assert_eq!(d.nullable(3), Some(false)); // literal constant

    Ok(())
}

#[sqlx_macros::test]
async fn it_describes_expression_from_empty_table() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    conn.execute("CREATE TEMP TABLE _temp_empty ( name TEXT NOT NULL, a INT )")
        .await?;

    let d = conn
        .describe("SELECT COUNT(*), a + 1, name, 5.12, 'Hello' FROM _temp_empty")
        .await?;

    assert_eq!(d.columns()[0].type_info().name(), "INTEGER");
    assert_eq!(d.nullable(0), Some(false)); // COUNT(*)

    assert_eq!(d.columns()[1].type_info().name(), "INTEGER");
    assert_eq!(d.nullable(1), None); // `a + 1` is potentially nullable but we don't know for sure currently

    assert_eq!(d.columns()[2].type_info().name(), "TEXT");
    assert_eq!(d.nullable(2), Some(false)); // `name` is not nullable

    assert_eq!(d.columns()[3].type_info().name(), "REAL");
    assert_eq!(d.nullable(3), Some(false)); // literal constant

    assert_eq!(d.columns()[4].type_info().name(), "TEXT");
    assert_eq!(d.nullable(4), Some(false)); // literal constant

    Ok(())
}

#[sqlx_macros::test]
async fn it_describes_expression_from_empty_table_with_star() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    conn.execute("CREATE TEMP TABLE _temp_empty ( name TEXT, a INT )")
        .await?;

    let d = conn
        .describe("SELECT *, 5, 'Hello' FROM _temp_empty")
        .await?;

    assert_eq!(d.columns()[0].type_info().name(), "TEXT");
    assert_eq!(d.columns()[1].type_info().name(), "INTEGER");
    assert_eq!(d.columns()[2].type_info().name(), "INTEGER");
    assert_eq!(d.columns()[3].type_info().name(), "TEXT");

    Ok(())
}

#[sqlx_macros::test]
async fn it_describes_insert() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    let d = conn
        .describe("INSERT INTO tweet (id, text) VALUES (2, 'Hello')")
        .await?;

    assert_eq!(d.columns().len(), 0);

    let d = conn
        .describe("INSERT INTO tweet (id, text) VALUES (2, 'Hello'); SELECT last_insert_rowid();")
        .await?;

    assert_eq!(d.columns().len(), 1);
    assert_eq!(d.columns()[0].type_info().name(), "INTEGER");
    assert_eq!(d.nullable(0), Some(false));

    Ok(())
}

#[sqlx_macros::test]
async fn it_describes_insert_with_read_only() -> anyhow::Result<()> {
    sqlx_test::setup_if_needed();

    let mut options: SqliteConnectOptions = env::var("DATABASE_URL")?.parse().unwrap();
    options = options.read_only(true);

    let mut conn = options.connect().await?;

    let d = conn
        .describe("INSERT INTO tweet (id, text) VALUES (2, 'Hello')")
        .await?;

    assert_eq!(d.columns().len(), 0);

    Ok(())
}

#[sqlx_macros::test]
async fn it_describes_bad_statement() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    let err = conn.describe("SELECT 1 FROM not_found").await.unwrap_err();
    let err = err
        .as_database_error()
        .unwrap()
        .downcast_ref::<SqliteError>();

    assert_eq!(err.message(), "no such table: not_found");
    assert_eq!(err.code().as_deref(), Some("1"));

    Ok(())
}

#[sqlx_macros::test]
async fn it_describes_left_join() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    let d = conn.describe("select accounts.id from accounts").await?;

    assert_eq!(d.column(0).type_info().name(), "INTEGER");
    assert_eq!(d.nullable(0), Some(false));

    let d = conn
        .describe("select tweet.id from accounts left join tweet on owner_id = accounts.id")
        .await?;

    assert_eq!(d.column(0).type_info().name(), "INTEGER");
    assert_eq!(d.nullable(0), Some(true));

    let d = conn
        .describe(
            "select tweet.id, accounts.id from accounts left join tweet on owner_id = accounts.id",
        )
        .await?;

    assert_eq!(d.column(0).type_info().name(), "INTEGER");
    assert_eq!(d.nullable(0), Some(true));

    assert_eq!(d.column(1).type_info().name(), "INTEGER");
    assert_eq!(d.nullable(1), Some(false));

    let d = conn
        .describe(
            "select tweet.id, accounts.id from accounts inner join tweet on owner_id = accounts.id",
        )
        .await?;

    assert_eq!(d.column(0).type_info().name(), "INTEGER");
    assert_eq!(d.nullable(0), Some(false));

    assert_eq!(d.column(1).type_info().name(), "INTEGER");
    assert_eq!(d.nullable(1), Some(false));

    Ok(())
}
