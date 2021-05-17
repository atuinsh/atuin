use futures::TryStreamExt;
use sqlx::mssql::Mssql;
use sqlx::{Column, Connection, Executor, MssqlConnection, Row, Statement, TypeInfo};
use sqlx_core::mssql::MssqlRow;
use sqlx_test::new;

#[sqlx_macros::test]
async fn it_connects() -> anyhow::Result<()> {
    let mut conn = new::<Mssql>().await?;

    conn.ping().await?;

    conn.close().await?;

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_select_expression() -> anyhow::Result<()> {
    let mut conn = new::<Mssql>().await?;

    let row: MssqlRow = conn.fetch_one("SELECT 4").await?;
    let v: i32 = row.try_get(0)?;

    assert_eq!(v, 4);

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_select_expression_by_name() -> anyhow::Result<()> {
    let mut conn = new::<Mssql>().await?;

    let row: MssqlRow = conn.fetch_one("SELECT 4 as _3").await?;
    let v: i32 = row.try_get("_3")?;

    assert_eq!(v, 4);

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_fail_to_connect() -> anyhow::Result<()> {
    let mut url = dotenv::var("DATABASE_URL")?;
    url = url.replace("Password", "NotPassword");

    let res = MssqlConnection::connect(&url).await;
    let err = res.unwrap_err();
    let err = err.into_database_error().unwrap();

    assert_eq!(err.message(), "Login failed for user \'sa\'.");

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_inspect_errors() -> anyhow::Result<()> {
    let mut conn = new::<Mssql>().await?;

    let res: Result<_, sqlx::Error> = sqlx::query("select f").execute(&mut conn).await;
    let err = res.unwrap_err();

    // can also do [as_database_error] or use `match ..`
    let err = err.into_database_error().unwrap();

    assert_eq!(err.message(), "Invalid column name 'f'.");

    Ok(())
}

#[sqlx_macros::test]
async fn it_maths() -> anyhow::Result<()> {
    let mut conn = new::<Mssql>().await?;

    let value = sqlx::query("SELECT 1 + @p1")
        .bind(5_i32)
        .try_map(|row: MssqlRow| row.try_get::<i32, _>(0))
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(6_i32, value);

    Ok(())
}

#[sqlx_macros::test]
async fn it_executes() -> anyhow::Result<()> {
    let mut conn = new::<Mssql>().await?;

    let _ = conn
        .execute(
            r#"
CREATE TABLE #users (id INTEGER PRIMARY KEY);
            "#,
        )
        .await?;

    for index in 1..=10_i32 {
        let done = sqlx::query("INSERT INTO #users (id) VALUES (@p1)")
            .bind(index * 2)
            .execute(&mut conn)
            .await?;

        assert_eq!(done.rows_affected(), 1);
    }

    let sum: i32 = sqlx::query("SELECT id FROM #users")
        .try_map(|row: MssqlRow| row.try_get::<i32, _>(0))
        .fetch(&mut conn)
        .try_fold(0_i32, |acc, x| async move { Ok(acc + x) })
        .await?;

    assert_eq!(sum, 110);

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_return_1000_rows() -> anyhow::Result<()> {
    let mut conn = new::<Mssql>().await?;

    let _ = conn
        .execute(
            r#"
CREATE TABLE #users (id INTEGER PRIMARY KEY);
            "#,
        )
        .await?;

    for index in 1..=1000_i32 {
        let done = sqlx::query("INSERT INTO #users (id) VALUES (@p1)")
            .bind(index * 2)
            .execute(&mut conn)
            .await?;

        assert_eq!(done.rows_affected(), 1);
    }

    let sum: i32 = sqlx::query("SELECT id FROM #users")
        .try_map(|row: MssqlRow| row.try_get::<i32, _>(0))
        .fetch(&mut conn)
        .try_fold(0_i32, |acc, x| async move { Ok(acc + x) })
        .await?;

    assert_eq!(sum, 1001000);

    Ok(())
}

#[sqlx_macros::test]
async fn it_selects_null() -> anyhow::Result<()> {
    let mut conn = new::<Mssql>().await?;

    let (_, val): (i32, Option<i32>) = sqlx::query_as("SELECT 5, NULL")
        .fetch_one(&mut conn)
        .await?;

    assert!(val.is_none());

    let val: Option<i32> = conn.fetch_one("SELECT 10, NULL").await?.try_get(1)?;

    assert!(val.is_none());

    Ok(())
}

#[sqlx_macros::test]
async fn it_binds_empty_string_and_null() -> anyhow::Result<()> {
    let mut conn = new::<Mssql>().await?;

    let (val, val2): (String, Option<String>) = sqlx::query_as("SELECT @p1, @p2")
        .bind("")
        .bind(None::<String>)
        .fetch_one(&mut conn)
        .await?;

    assert!(val.is_empty());
    assert!(val2.is_none());

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_work_with_transactions() -> anyhow::Result<()> {
    let mut conn = new::<Mssql>().await?;

    conn.execute("IF OBJECT_ID('_sqlx_users_1922', 'U') IS NULL CREATE TABLE _sqlx_users_1922 (id INTEGER PRIMARY KEY)")
        .await?;

    conn.execute("DELETE FROM _sqlx_users_1922").await?;

    // begin .. rollback

    let mut tx = conn.begin().await?;

    sqlx::query("INSERT INTO _sqlx_users_1922 (id) VALUES ($1)")
        .bind(10_i32)
        .execute(&mut tx)
        .await?;

    tx.rollback().await?;

    let (count,): (i32,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_users_1922")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(count, 0);

    // begin .. commit

    let mut tx = conn.begin().await?;

    sqlx::query("INSERT INTO _sqlx_users_1922 (id) VALUES (@p1)")
        .bind(10_i32)
        .execute(&mut tx)
        .await?;

    tx.commit().await?;

    let (count,): (i32,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_users_1922")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(count, 1);

    // begin .. (drop)

    {
        let mut tx = conn.begin().await?;

        sqlx::query("INSERT INTO _sqlx_users_1922 (id) VALUES (@p1)")
            .bind(20_i32)
            .execute(&mut tx)
            .await?;
    }

    conn = new::<Mssql>().await?;

    let (count,): (i32,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_users_1922")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(count, 1);

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_work_with_nested_transactions() -> anyhow::Result<()> {
    let mut conn = new::<Mssql>().await?;

    conn.execute("IF OBJECT_ID('_sqlx_users_2523', 'U') IS NULL CREATE TABLE _sqlx_users_2523 (id INTEGER PRIMARY KEY)")
        .await?;

    conn.execute("DELETE FROM _sqlx_users_2523").await?;

    // begin
    let mut tx = conn.begin().await?;

    // insert a user
    sqlx::query("INSERT INTO _sqlx_users_2523 (id) VALUES (@p1)")
        .bind(50_i32)
        .execute(&mut tx)
        .await?;

    // begin once more
    let mut tx2 = tx.begin().await?;

    // insert another user
    sqlx::query("INSERT INTO _sqlx_users_2523 (id) VALUES (@p1)")
        .bind(10_i32)
        .execute(&mut tx2)
        .await?;

    // never mind, rollback
    tx2.rollback().await?;

    // did we really?
    let (count,): (i32,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_users_2523")
        .fetch_one(&mut tx)
        .await?;

    assert_eq!(count, 1);

    // actually, commit
    tx.commit().await?;

    // did we really?
    let (count,): (i32,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_users_2523")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(count, 1);

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_prepare_then_execute() -> anyhow::Result<()> {
    let mut conn = new::<Mssql>().await?;
    let mut tx = conn.begin().await?;

    let tweet_id: i64 = sqlx::query_scalar(
        "INSERT INTO tweet ( id, text ) OUTPUT INSERTED.id VALUES ( 50, 'Hello, World' )",
    )
    .fetch_one(&mut tx)
    .await?;

    let statement = tx.prepare("SELECT * FROM tweet WHERE id = @p1").await?;

    assert_eq!(statement.column(0).name(), "id");
    assert_eq!(statement.column(1).name(), "text");
    assert_eq!(statement.column(2).name(), "is_sent");
    assert_eq!(statement.column(3).name(), "owner_id");

    assert_eq!(statement.column(0).type_info().name(), "BIGINT");
    assert_eq!(statement.column(1).type_info().name(), "NVARCHAR");
    assert_eq!(statement.column(2).type_info().name(), "TINYINT");
    assert_eq!(statement.column(3).type_info().name(), "BIGINT");

    let row = statement.query().bind(tweet_id).fetch_one(&mut tx).await?;
    let tweet_text: String = row.try_get("text")?;

    assert_eq!(tweet_text, "Hello, World");

    Ok(())
}
