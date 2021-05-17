use futures::TryStreamExt;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{
    query, sqlite::Sqlite, sqlite::SqliteRow, Column, Connection, Executor, Row, SqliteConnection,
    SqlitePool, Statement, TypeInfo,
};
use sqlx_test::new;

#[sqlx_macros::test]
async fn it_connects() -> anyhow::Result<()> {
    Ok(new::<Sqlite>().await?.ping().await?)
}

#[sqlx_macros::test]
async fn it_fetches_and_inflates_row() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    // process rows, one-at-a-time
    // this reuses the memory of the row

    {
        let expected = [15, 39, 51];
        let mut i = 0;
        let mut s = conn.fetch("SELECT 15 UNION SELECT 51 UNION SELECT 39");

        while let Some(row) = s.try_next().await? {
            let v1 = row.get::<i32, _>(0);
            assert_eq!(expected[i], v1);
            i += 1;
        }
    }

    // same query, but fetch all rows at once
    // this triggers the internal inflation

    let rows = conn
        .fetch_all("SELECT 15 UNION SELECT 51 UNION SELECT 39")
        .await?;

    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].get::<i32, _>(0), 15);
    assert_eq!(rows[1].get::<i32, _>(0), 39);
    assert_eq!(rows[2].get::<i32, _>(0), 51);

    // same query but fetch the first row a few times from a non-persistent query
    // these rows should be immediately inflated

    let row1 = conn
        .fetch_one("SELECT 15 UNION SELECT 51 UNION SELECT 39")
        .await?;

    assert_eq!(row1.get::<i32, _>(0), 15);

    let row2 = conn
        .fetch_one("SELECT 15 UNION SELECT 51 UNION SELECT 39")
        .await?;

    assert_eq!(row1.get::<i32, _>(0), 15);
    assert_eq!(row2.get::<i32, _>(0), 15);

    // same query (again) but make it persistent
    // and fetch the first row a few times

    let row1 = conn
        .fetch_one(query("SELECT 15 UNION SELECT 51 UNION SELECT 39"))
        .await?;

    assert_eq!(row1.get::<i32, _>(0), 15);

    let row2 = conn
        .fetch_one(query("SELECT 15 UNION SELECT 51 UNION SELECT 39"))
        .await?;

    assert_eq!(row1.get::<i32, _>(0), 15);
    assert_eq!(row2.get::<i32, _>(0), 15);

    Ok(())
}

#[sqlx_macros::test]
async fn it_maths() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    let value = sqlx::query("select 1 + ?1")
        .bind(5_i32)
        .try_map(|row: SqliteRow| row.try_get::<i32, _>(0))
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(6i32, value);

    Ok(())
}

#[sqlx_macros::test]
async fn test_bind_multiple_statements_multiple_values() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    let values: Vec<i32> = sqlx::query_scalar::<_, i32>("select ?; select ?")
        .bind(5_i32)
        .bind(15_i32)
        .fetch_all(&mut conn)
        .await?;

    assert_eq!(values.len(), 2);
    assert_eq!(values[0], 5);
    assert_eq!(values[1], 15);

    Ok(())
}

#[sqlx_macros::test]
async fn test_bind_multiple_statements_same_value() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    let values: Vec<i32> = sqlx::query_scalar::<_, i32>("select ?1; select ?1")
        .bind(25_i32)
        .fetch_all(&mut conn)
        .await?;

    assert_eq!(values.len(), 2);
    assert_eq!(values[0], 25);
    assert_eq!(values[1], 25);

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_describe_with_pragma() -> anyhow::Result<()> {
    use sqlx::{Decode, TypeInfo, ValueRef};

    let mut conn = new::<Sqlite>().await?;

    let defaults = sqlx::query("pragma table_info (tweet)")
        .try_map(|row: SqliteRow| {
            let val = row.try_get_raw("dflt_value")?;
            let ty = val.type_info().clone().into_owned();

            let val: Option<i32> = Decode::<Sqlite>::decode(val).map_err(sqlx::Error::Decode)?;

            if val.is_some() {
                assert_eq!(ty.name(), "TEXT");
            }

            Ok(val)
        })
        .fetch_all(&mut conn)
        .await?;

    assert_eq!(defaults[0], None);
    assert_eq!(defaults[2], Some(0));

    Ok(())
}

#[sqlx_macros::test]
async fn it_binds_positional_parameters_issue_467() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    let row: (i32, i32, i32, i32) = sqlx::query_as("select ?1, ?1, ?3, ?2")
        .bind(5_i32)
        .bind(500_i32)
        .bind(1020_i32)
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(row.0, 5);
    assert_eq!(row.1, 5);
    assert_eq!(row.2, 1020);
    assert_eq!(row.3, 500);

    Ok(())
}

#[sqlx_macros::test]
async fn it_fetches_in_loop() -> anyhow::Result<()> {
    // this is trying to check for any data races
    // there were a few that triggered *sometimes* while building out StatementWorker
    for _ in 0..1000_usize {
        let mut conn = new::<Sqlite>().await?;
        let v: Vec<(i32,)> = sqlx::query_as("SELECT 1").fetch_all(&mut conn).await?;

        assert_eq!(v[0].0, 1);
    }

    Ok(())
}

#[sqlx_macros::test]
async fn it_executes_with_pool() -> anyhow::Result<()> {
    let pool: SqlitePool = SqlitePoolOptions::new()
        .min_connections(2)
        .max_connections(2)
        .test_before_acquire(false)
        .connect(&dotenv::var("DATABASE_URL")?)
        .await?;

    let rows = pool.fetch_all("SELECT 1; SElECT 2").await?;

    assert_eq!(rows.len(), 2);

    Ok(())
}

#[sqlx_macros::test]
async fn it_opens_in_memory() -> anyhow::Result<()> {
    // If the filename is ":memory:", then a private, temporary in-memory database
    // is created for the connection.
    let _ = SqliteConnection::connect(":memory:").await?;

    Ok(())
}

#[sqlx_macros::test]
async fn it_opens_temp_on_disk() -> anyhow::Result<()> {
    // If the filename is an empty string, then a private, temporary on-disk database will
    // be created.
    let _ = SqliteConnection::connect("").await?;

    Ok(())
}

#[sqlx_macros::test]
async fn it_fails_to_parse() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;
    let res = conn.execute("SEELCT 1").await;

    assert!(res.is_err());

    let err = res.unwrap_err().to_string();

    assert_eq!(
        "error returned from database: near \"SEELCT\": syntax error",
        err
    );

    Ok(())
}

#[sqlx_macros::test]
async fn it_handles_empty_queries() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;
    let done = conn.execute("").await?;

    assert_eq!(done.rows_affected(), 0);

    Ok(())
}

#[sqlx_macros::test]
fn it_binds_parameters() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    let v: i32 = sqlx::query_scalar("SELECT ?")
        .bind(10_i32)
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(v, 10);

    let v: (i32, i32) = sqlx::query_as("SELECT ?1, ?")
        .bind(10_i32)
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(v.0, 10);
    assert_eq!(v.1, 10);

    Ok(())
}

#[sqlx_macros::test]
fn it_binds_dollar_parameters() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    let v: (i32, i32) = sqlx::query_as("SELECT $1, $2")
        .bind(10_i32)
        .bind(11_i32)
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(v.0, 10);
    assert_eq!(v.1, 11);

    Ok(())
}

#[sqlx_macros::test]
async fn it_executes_queries() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    let _ = conn
        .execute(
            r#"
CREATE TEMPORARY TABLE users (id INTEGER PRIMARY KEY)
            "#,
        )
        .await?;

    for index in 1..=10_i32 {
        let done = sqlx::query("INSERT INTO users (id) VALUES (?)")
            .bind(index * 2)
            .execute(&mut conn)
            .await?;

        assert_eq!(done.rows_affected(), 1);
    }

    let sum: i32 = sqlx::query_as("SELECT id FROM users")
        .fetch(&mut conn)
        .try_fold(0_i32, |acc, (x,): (i32,)| async move { Ok(acc + x) })
        .await?;

    assert_eq!(sum, 110);

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_execute_multiple_statements() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    let done = conn
        .execute(
            r#"
CREATE TEMPORARY TABLE users (id INTEGER PRIMARY KEY, other INTEGER);
INSERT INTO users DEFAULT VALUES;
            "#,
        )
        .await?;

    assert_eq!(done.rows_affected(), 1);

    for index in 2..5_i32 {
        let (id, other): (i32, i32) = sqlx::query_as(
            r#"
INSERT INTO users (other) VALUES (?);
SELECT id, other FROM users WHERE id = last_insert_rowid();
            "#,
        )
        .bind(index)
        .fetch_one(&mut conn)
        .await?;

        assert_eq!(id, index);
        assert_eq!(other, index);
    }

    Ok(())
}

#[sqlx_macros::test]
async fn it_interleaves_reads_and_writes() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    let mut cursor = conn.fetch(
        "
CREATE TABLE IF NOT EXISTS _sqlx_test (
    id INT PRIMARY KEY,
    text TEXT NOT NULL
);

SELECT 'Hello World' as _1;

INSERT INTO _sqlx_test (text) VALUES ('this is a test');

SELECT id, text FROM _sqlx_test;
    ",
    );

    let row = cursor.try_next().await?.unwrap();

    assert!("Hello World" == row.try_get::<&str, _>("_1")?);

    let row = cursor.try_next().await?.unwrap();

    let id: i64 = row.try_get("id")?;
    let text: &str = row.try_get("text")?;

    assert_eq!(0, id);
    assert_eq!("this is a test", text);

    Ok(())
}

#[sqlx_macros::test]
async fn it_supports_collations() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    conn.create_collation("test_collation", |l, r| l.cmp(r).reverse())?;

    let _ = conn
        .execute(
            r#"
CREATE TEMPORARY TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL COLLATE test_collation)
            "#,
        )
        .await?;

    sqlx::query("INSERT INTO users (name) VALUES (?)")
        .bind("a")
        .execute(&mut conn)
        .await?;
    sqlx::query("INSERT INTO users (name) VALUES (?)")
        .bind("b")
        .execute(&mut conn)
        .await?;

    let row: SqliteRow = conn
        .fetch_one("SELECT name FROM users ORDER BY name ASC")
        .await?;
    let name: &str = row.try_get(0)?;

    assert_eq!(name, "b");

    Ok(())
}

#[sqlx_macros::test]
async fn it_caches_statements() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    // Initial PRAGMAs are not cached as we are not going to execute
    // them more than once.
    assert_eq!(0, conn.cached_statements_size());

    // `&str` queries are not persistent.
    let row = conn.fetch_one("SELECT 100 AS val").await?;
    let val: i32 = row.get("val");
    assert_eq!(val, 100);
    assert_eq!(0, conn.cached_statements_size());

    // `Query` is persistent by default.
    let mut conn = new::<Sqlite>().await?;
    for i in 0..2 {
        let row = sqlx::query("SELECT ? AS val")
            .bind(i)
            .fetch_one(&mut conn)
            .await?;

        let val: i32 = row.get("val");

        assert_eq!(i, val);
    }
    assert_eq!(1, conn.cached_statements_size());

    // Cache can be cleared.
    conn.clear_cached_statements().await?;
    assert_eq!(0, conn.cached_statements_size());

    // `Query` is not persistent if `.persistent(false)` is used
    // explicity.
    let mut conn = new::<Sqlite>().await?;
    for i in 0..2 {
        let row = sqlx::query("SELECT ? AS val")
            .bind(i)
            .persistent(false)
            .fetch_one(&mut conn)
            .await?;

        let val: i32 = row.get("val");

        assert_eq!(i, val);
    }
    assert_eq!(0, conn.cached_statements_size());

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_prepare_then_execute() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;
    let mut tx = conn.begin().await?;

    let _ = sqlx::query("INSERT INTO tweet ( id, text ) VALUES ( 2, 'Hello, World' )")
        .execute(&mut tx)
        .await?;

    let tweet_id: i32 = 2;

    let statement = tx.prepare("SELECT * FROM tweet WHERE id = ?1").await?;

    assert_eq!(statement.column(0).name(), "id");
    assert_eq!(statement.column(1).name(), "text");
    assert_eq!(statement.column(2).name(), "is_sent");
    assert_eq!(statement.column(3).name(), "owner_id");

    assert_eq!(statement.column(0).type_info().name(), "INTEGER");
    assert_eq!(statement.column(1).type_info().name(), "TEXT");
    assert_eq!(statement.column(2).type_info().name(), "BOOLEAN");
    assert_eq!(statement.column(3).type_info().name(), "INTEGER");

    let row = statement.query().bind(tweet_id).fetch_one(&mut tx).await?;
    let tweet_text: &str = row.try_get("text")?;

    assert_eq!(tweet_text, "Hello, World");

    Ok(())
}

#[sqlx_macros::test]
async fn it_resets_prepared_statement_after_fetch_one() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    conn.execute("CREATE TEMPORARY TABLE foobar (id INTEGER)")
        .await?;
    conn.execute("INSERT INTO foobar VALUES (42)").await?;

    let r = sqlx::query("SELECT id FROM foobar")
        .fetch_one(&mut conn)
        .await?;
    let x: i32 = r.try_get("id")?;
    assert_eq!(x, 42);

    conn.execute("DROP TABLE foobar").await?;

    Ok(())
}

#[sqlx_macros::test]
async fn it_resets_prepared_statement_after_fetch_many() -> anyhow::Result<()> {
    let mut conn = new::<Sqlite>().await?;

    conn.execute("CREATE TEMPORARY TABLE foobar (id INTEGER)")
        .await?;
    conn.execute("INSERT INTO foobar VALUES (42)").await?;
    conn.execute("INSERT INTO foobar VALUES (43)").await?;

    let mut rows = sqlx::query("SELECT id FROM foobar").fetch(&mut conn);
    let row = rows.try_next().await?.unwrap();
    let x: i32 = row.try_get("id")?;
    assert_eq!(x, 42);
    drop(rows);

    conn.execute("DROP TABLE foobar").await?;

    Ok(())
}
