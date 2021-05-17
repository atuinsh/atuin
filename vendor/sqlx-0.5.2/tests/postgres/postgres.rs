use futures::TryStreamExt;
use sqlx::postgres::{
    PgConnectOptions, PgConnection, PgDatabaseError, PgErrorPosition, PgSeverity,
};
use sqlx::postgres::{PgPoolOptions, PgRow, Postgres};
use sqlx::{Column, Connection, Executor, Row, Statement, TypeInfo};
use sqlx_test::{new, setup_if_needed};
use std::env;
use std::time::Duration;

#[sqlx_macros::test]
async fn it_connects() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    let value = sqlx::query("select 1 + 1")
        .try_map(|row: PgRow| row.try_get::<i32, _>(0))
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(2i32, value);

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_select_void() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    // pg_notify just happens to be a function that returns void
    let _: () = sqlx::query_scalar("select pg_notify('chan', 'message');")
        .fetch_one(&mut conn)
        .await?;

    Ok(())
}

#[sqlx_macros::test]
async fn it_pings() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    conn.ping().await?;

    Ok(())
}

#[sqlx_macros::test]
async fn it_maths() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    let value = sqlx::query("select 1 + $1::int")
        .bind(5_i32)
        .try_map(|row: PgRow| row.try_get::<i32, _>(0))
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(6i32, value);

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_inspect_errors() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    let res: Result<_, sqlx::Error> = sqlx::query("select f").execute(&mut conn).await;
    let err = res.unwrap_err();

    // can also do [as_database_error] or use `match ..`
    let err = err.into_database_error().unwrap();

    assert_eq!(err.message(), "column \"f\" does not exist");
    assert_eq!(err.code().as_deref(), Some("42703"));

    // can also do [downcast_ref]
    let err: Box<PgDatabaseError> = err.downcast();

    assert_eq!(err.severity(), PgSeverity::Error);
    assert_eq!(err.message(), "column \"f\" does not exist");
    assert_eq!(err.code(), "42703");
    assert_eq!(err.position(), Some(PgErrorPosition::Original(8)));
    assert_eq!(err.routine(), Some("errorMissingColumn"));
    assert_eq!(err.constraint(), None);

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_inspect_constraint_errors() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    let res: Result<_, sqlx::Error> =
        sqlx::query("INSERT INTO products VALUES (1, 'Product 1', 0);")
            .execute(&mut conn)
            .await;
    let err = res.unwrap_err();

    // can also do [as_database_error] or use `match ..`
    let err = err.into_database_error().unwrap();

    assert_eq!(
        err.message(),
        "new row for relation \"products\" violates check constraint \"products_price_check\""
    );
    assert_eq!(err.code().as_deref(), Some("23514"));

    // can also do [downcast_ref]
    let err: Box<PgDatabaseError> = err.downcast();

    assert_eq!(err.severity(), PgSeverity::Error);
    assert_eq!(
        err.message(),
        "new row for relation \"products\" violates check constraint \"products_price_check\""
    );
    assert_eq!(err.code(), "23514");
    assert_eq!(err.position(), None);
    assert_eq!(err.routine(), Some("ExecConstraints"));
    assert_eq!(err.constraint(), Some("products_price_check"));

    Ok(())
}

#[sqlx_macros::test]
async fn it_executes() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    let _ = conn
        .execute(
            r#"
CREATE TEMPORARY TABLE users (id INTEGER PRIMARY KEY);
            "#,
        )
        .await?;

    for index in 1..=10_i32 {
        let done = sqlx::query("INSERT INTO users (id) VALUES ($1)")
            .bind(index)
            .execute(&mut conn)
            .await?;

        assert_eq!(done.rows_affected(), 1);
    }

    let sum: i32 = sqlx::query("SELECT id FROM users")
        .try_map(|row: PgRow| row.try_get::<i32, _>(0))
        .fetch(&mut conn)
        .try_fold(0_i32, |acc, x| async move { Ok(acc + x) })
        .await?;

    assert_eq!(sum, 55);

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_nest_map() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    let res = sqlx::query("SELECT 5")
        .map(|row: PgRow| row.get(0))
        .map(|int: i32| int.to_string())
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(res, "5");

    Ok(())
}

#[cfg(feature = "json")]
#[sqlx_macros::test]
async fn it_describes_and_inserts_json_and_jsonb() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    let _ = conn
        .execute(
            r#"
CREATE TEMPORARY TABLE json_stuff (obj json, obj2 jsonb);
            "#,
        )
        .await?;

    let query = "INSERT INTO json_stuff (obj, obj2) VALUES ($1, $2)";
    let _ = conn.describe(query).await?;

    let done = sqlx::query(query)
        .bind(serde_json::json!({ "a": "a" }))
        .bind(serde_json::json!({ "a": "a" }))
        .execute(&mut conn)
        .await?;

    assert_eq!(done.rows_affected(), 1);

    Ok(())
}

#[sqlx_macros::test]
async fn it_works_with_cache_disabled() -> anyhow::Result<()> {
    setup_if_needed();

    let mut url = url::Url::parse(&env::var("DATABASE_URL")?)?;
    url.query_pairs_mut()
        .append_pair("statement-cache-capacity", "0");

    let mut conn = PgConnection::connect(url.as_ref()).await?;

    for index in 1..=10_i32 {
        let _ = sqlx::query("SELECT $1")
            .bind(index)
            .execute(&mut conn)
            .await?;
    }

    Ok(())
}

#[sqlx_macros::test]
async fn it_executes_with_pool() -> anyhow::Result<()> {
    let pool = sqlx_test::pool::<Postgres>().await?;

    let rows = pool.fetch_all("SELECT 1; SElECT 2").await?;

    assert_eq!(rows.len(), 2);

    Ok(())
}

// https://github.com/launchbadge/sqlx/issues/104
#[sqlx_macros::test]
async fn it_can_return_interleaved_nulls_issue_104() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    let tuple = sqlx::query("SELECT NULL, 10::INT, NULL, 20::INT, NULL, 40::INT, NULL, 80::INT")
        .map(|row: PgRow| {
            (
                row.get::<Option<i32>, _>(0),
                row.get::<Option<i32>, _>(1),
                row.get::<Option<i32>, _>(2),
                row.get::<Option<i32>, _>(3),
                row.get::<Option<i32>, _>(4),
                row.get::<Option<i32>, _>(5),
                row.get::<Option<i32>, _>(6),
                row.get::<Option<i32>, _>(7),
            )
        })
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(tuple.0, None);
    assert_eq!(tuple.1, Some(10));
    assert_eq!(tuple.2, None);
    assert_eq!(tuple.3, Some(20));
    assert_eq!(tuple.4, None);
    assert_eq!(tuple.5, Some(40));
    assert_eq!(tuple.6, None);
    assert_eq!(tuple.7, Some(80));

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_fail_and_recover() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    for i in 0..10 {
        // make a query that will fail
        let res = conn
            .execute("INSERT INTO not_found (column) VALUES (10)")
            .await;

        assert!(res.is_err());

        // now try and use the connection
        let val: i32 = conn
            .fetch_one(&*format!("SELECT {}::int4", i))
            .await?
            .get(0);

        assert_eq!(val, i);
    }

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_fail_and_recover_with_pool() -> anyhow::Result<()> {
    let pool = sqlx_test::pool::<Postgres>().await?;

    for i in 0..10 {
        // make a query that will fail
        let res = pool
            .execute("INSERT INTO not_found (column) VALUES (10)")
            .await;

        assert!(res.is_err());

        // now try and use the connection
        let val: i32 = pool
            .fetch_one(&*format!("SELECT {}::int4", i))
            .await?
            .get(0);

        assert_eq!(val, i);
    }

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_query_scalar() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    let scalar: i32 = sqlx::query_scalar("SELECT 42").fetch_one(&mut conn).await?;
    assert_eq!(scalar, 42);

    let scalar: Option<i32> = sqlx::query_scalar("SELECT 42").fetch_one(&mut conn).await?;
    assert_eq!(scalar, Some(42));

    let scalar: Option<i32> = sqlx::query_scalar("SELECT NULL")
        .fetch_one(&mut conn)
        .await?;
    assert_eq!(scalar, None);

    let scalar: Option<i64> = sqlx::query_scalar("SELECT 42::bigint")
        .fetch_optional(&mut conn)
        .await?;
    assert_eq!(scalar, Some(42));

    let scalar: Option<i16> = sqlx::query_scalar("").fetch_optional(&mut conn).await?;
    assert_eq!(scalar, None);

    Ok(())
}

#[sqlx_macros::test]
/// This is seperate from `it_can_query_scalar` because while implementing it I ran into a
/// bug which that prevented `Vec<i32>` from compiling but allowed Vec<Option<i32>>.
async fn it_can_query_all_scalar() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    let scalar: Vec<i32> = sqlx::query_scalar("SELECT $1")
        .bind(42)
        .fetch_all(&mut conn)
        .await?;
    assert_eq!(scalar, vec![42]);

    let scalar: Vec<Option<i32>> = sqlx::query_scalar("SELECT $1 UNION ALL SELECT NULL")
        .bind(42)
        .fetch_all(&mut conn)
        .await?;
    assert_eq!(scalar, vec![Some(42), None]);

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_work_with_transactions() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    conn.execute("CREATE TABLE IF NOT EXISTS _sqlx_users_1922 (id INTEGER PRIMARY KEY)")
        .await?;

    conn.execute("TRUNCATE _sqlx_users_1922").await?;

    // begin .. rollback

    let mut tx = conn.begin().await?;

    sqlx::query("INSERT INTO _sqlx_users_1922 (id) VALUES ($1)")
        .bind(10_i32)
        .execute(&mut tx)
        .await?;

    tx.rollback().await?;

    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_users_1922")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(count, 0);

    // begin .. commit

    let mut tx = conn.begin().await?;

    sqlx::query("INSERT INTO _sqlx_users_1922 (id) VALUES ($1)")
        .bind(10_i32)
        .execute(&mut tx)
        .await?;

    tx.commit().await?;

    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_users_1922")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(count, 1);

    // begin .. (drop)

    {
        let mut tx = conn.begin().await?;

        sqlx::query("INSERT INTO _sqlx_users_1922 (id) VALUES ($1)")
            .bind(20_i32)
            .execute(&mut tx)
            .await?;
    }

    conn = new::<Postgres>().await?;

    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_users_1922")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(count, 1);

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_work_with_nested_transactions() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    conn.execute("CREATE TABLE IF NOT EXISTS _sqlx_users_2523 (id INTEGER PRIMARY KEY)")
        .await?;

    conn.execute("TRUNCATE _sqlx_users_2523").await?;

    // begin
    let mut tx = conn.begin().await?; // transaction

    // insert a user
    sqlx::query("INSERT INTO _sqlx_users_2523 (id) VALUES ($1)")
        .bind(50_i32)
        .execute(&mut tx)
        .await?;

    // begin once more
    let mut tx2 = tx.begin().await?; // savepoint

    // insert another user
    sqlx::query("INSERT INTO _sqlx_users_2523 (id) VALUES ($1)")
        .bind(10_i32)
        .execute(&mut tx2)
        .await?;

    // never mind, rollback
    tx2.rollback().await?; // roll that one back

    // did we really?
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_users_2523")
        .fetch_one(&mut tx)
        .await?;

    assert_eq!(count, 1);

    // actually, commit
    tx.commit().await?;

    // did we really?
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_users_2523")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(count, 1);

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_drop_multiple_transactions() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    conn.execute("CREATE TABLE IF NOT EXISTS _sqlx_users_3952 (id INTEGER PRIMARY KEY)")
        .await?;

    conn.execute("TRUNCATE _sqlx_users_3952").await?;

    // begin .. (drop)

    // run 2 times to see what happens if we drop transactions repeatedly
    for _ in 0..2 {
        {
            let mut tx = conn.begin().await?;

            // do actually something before dropping
            let _user = sqlx::query("INSERT INTO _sqlx_users_3952 (id) VALUES ($1) RETURNING id")
                .bind(20_i32)
                .fetch_one(&mut tx)
                .await?;
        }

        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_users_3952")
            .fetch_one(&mut conn)
            .await?;

        assert_eq!(count, 0);
    }

    Ok(())
}

// run with `cargo test --features postgres -- --ignored --nocapture pool_smoke_test`
#[ignore]
#[sqlx_macros::test]
async fn pool_smoke_test() -> anyhow::Result<()> {
    use futures::{future, task::Poll, Future};

    eprintln!("starting pool");

    let pool = PgPoolOptions::new()
        .connect_timeout(Duration::from_secs(30))
        .min_connections(5)
        .max_connections(10)
        .connect(&dotenv::var("DATABASE_URL")?)
        .await?;

    // spin up more tasks than connections available, and ensure we don't deadlock
    for i in 0..20 {
        let pool = pool.clone();
        sqlx_rt::spawn(async move {
            loop {
                if let Err(e) = sqlx::query("select 1 + 1").execute(&pool).await {
                    eprintln!("pool task {} dying due to {}", i, e);
                    break;
                }
            }
        });
    }

    for _ in 0..5 {
        let pool = pool.clone();
        sqlx_rt::spawn(async move {
            while !pool.is_closed() {
                let acquire = pool.acquire();
                futures::pin_mut!(acquire);

                // poll the acquire future once to put the waiter in the queue
                future::poll_fn(move |cx| {
                    let _ = acquire.as_mut().poll(cx);
                    Poll::Ready(())
                })
                .await;

                sqlx_rt::yield_now().await;
            }
        });
    }

    eprintln!("sleeping for 30 seconds");

    sqlx_rt::sleep(Duration::from_secs(30)).await;

    // assert_eq!(pool.size(), 10);

    eprintln!("closing pool");

    sqlx_rt::timeout(Duration::from_secs(30), pool.close()).await?;

    eprintln!("pool closed successfully");

    Ok(())
}

#[sqlx_macros::test]
async fn test_invalid_query() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    conn.execute("definitely not a correct query")
        .await
        .unwrap_err();

    let mut s = conn.fetch("select 1");
    let row = s.try_next().await?.unwrap();

    assert_eq!(row.get::<i32, _>(0), 1i32);

    Ok(())
}

/// Tests the edge case of executing a completely empty query string.
///
/// This gets flagged as an `EmptyQueryResponse` in Postgres. We
/// catch this and just return no rows.
#[sqlx_macros::test]
async fn test_empty_query() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;
    let done = conn.execute("").await?;

    assert_eq!(done.rows_affected(), 0);

    Ok(())
}

/// Test a simple select expression. This should return the row.
#[sqlx_macros::test]
async fn test_select_expression() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    let mut s = conn.fetch("SELECT 5");
    let row = s.try_next().await?.unwrap();

    assert!(5i32 == row.try_get::<i32, _>(0)?);

    Ok(())
}

/// Test that we can interleave reads and writes to the database
/// in one simple query. Using the `Cursor` API we should be
/// able to fetch from both queries in sequence.
#[sqlx_macros::test]
async fn test_multi_read_write() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    let mut s = conn.fetch(
        "
CREATE TABLE IF NOT EXISTS _sqlx_test_postgres_5112 (
    id BIGSERIAL PRIMARY KEY,
    text TEXT NOT NULL
);

SELECT 'Hello World' as _1;

INSERT INTO _sqlx_test_postgres_5112 (text) VALUES ('this is a test');

SELECT id, text FROM _sqlx_test_postgres_5112;
    ",
    );

    let row = s.try_next().await?.unwrap();

    assert!("Hello World" == row.try_get::<&str, _>("_1")?);

    let row = s.try_next().await?.unwrap();

    let id: i64 = row.try_get("id")?;
    let text: &str = row.try_get("text")?;

    assert_eq!(1_i64, id);
    assert_eq!("this is a test", text);

    Ok(())
}

#[sqlx_macros::test]
async fn it_caches_statements() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    for i in 0..2 {
        let row = sqlx::query("SELECT $1 AS val")
            .bind(i)
            .persistent(true)
            .fetch_one(&mut conn)
            .await?;

        let val: u32 = row.get("val");

        assert_eq!(i, val);
    }

    assert_eq!(1, conn.cached_statements_size());
    conn.clear_cached_statements().await?;
    assert_eq!(0, conn.cached_statements_size());

    for i in 0..2 {
        let row = sqlx::query("SELECT $1 AS val")
            .bind(i)
            .persistent(false)
            .fetch_one(&mut conn)
            .await?;

        let val: u32 = row.get("val");

        assert_eq!(i, val);
    }

    assert_eq!(0, conn.cached_statements_size());

    Ok(())
}

#[sqlx_macros::test]
async fn it_closes_statement_from_cache_issue_470() -> anyhow::Result<()> {
    sqlx_test::setup_if_needed();

    let mut options: PgConnectOptions = env::var("DATABASE_URL")?.parse().unwrap();

    // a capacity of 1 means that before each statement (after the first)
    // we will close the previous statement
    options = options.statement_cache_capacity(1);

    let mut conn = PgConnection::connect_with(&options).await?;

    for i in 0..5 {
        let row = sqlx::query(&*format!("SELECT {}::int4 AS val", i))
            .fetch_one(&mut conn)
            .await?;

        let val: i32 = row.get("val");

        assert_eq!(i, val);
    }

    assert_eq!(1, conn.cached_statements_size());

    Ok(())
}

#[sqlx_macros::test]
async fn it_sets_application_name() -> anyhow::Result<()> {
    sqlx_test::setup_if_needed();

    let mut options: PgConnectOptions = env::var("DATABASE_URL")?.parse().unwrap();
    options = options.application_name("some-name");

    let mut conn = PgConnection::connect_with(&options).await?;

    let row = sqlx::query("select current_setting('application_name') as app_name")
        .fetch_one(&mut conn)
        .await?;

    let val: String = row.get("app_name");

    assert_eq!("some-name", &val);

    Ok(())
}

#[sqlx_macros::test]
async fn it_can_handle_parameter_status_message_issue_484() -> anyhow::Result<()> {
    new::<Postgres>().await?.execute("SET NAMES 'UTF8'").await?;
    Ok(())
}

#[sqlx_macros::test]
async fn it_can_prepare_then_execute() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;
    let mut tx = conn.begin().await?;

    let tweet_id: i64 =
        sqlx::query_scalar("INSERT INTO tweet ( text ) VALUES ( 'Hello, World' ) RETURNING id")
            .fetch_one(&mut tx)
            .await?;

    let statement = tx.prepare("SELECT * FROM tweet WHERE id = $1").await?;

    assert_eq!(statement.column(0).name(), "id");
    assert_eq!(statement.column(1).name(), "created_at");
    assert_eq!(statement.column(2).name(), "text");
    assert_eq!(statement.column(3).name(), "owner_id");

    assert_eq!(statement.column(0).type_info().name(), "INT8");
    assert_eq!(statement.column(1).type_info().name(), "TIMESTAMPTZ");
    assert_eq!(statement.column(2).type_info().name(), "TEXT");
    assert_eq!(statement.column(3).type_info().name(), "INT8");

    let row = statement.query().bind(tweet_id).fetch_one(&mut tx).await?;
    let tweet_text: &str = row.try_get("text")?;

    assert_eq!(tweet_text, "Hello, World");

    Ok(())
}

// repro is more reliable with the basic scheduler used by `#[tokio::test]`
#[cfg(feature = "_rt-tokio")]
#[tokio::test]
async fn test_issue_622() -> anyhow::Result<()> {
    use std::time::Instant;

    setup_if_needed();

    let pool = PgPoolOptions::new()
        .max_connections(1) // also fails with higher counts, e.g. 5
        .connect(&std::env::var("DATABASE_URL").unwrap())
        .await?;

    println!("pool state: {:?}", pool);

    let mut handles = vec![];

    // given repro spawned 100 tasks but I found it reliably reproduced with 3
    for i in 0..3 {
        let pool = pool.clone();

        handles.push(sqlx_rt::spawn(async move {
            {
                let mut conn = pool.acquire().await.unwrap();

                let _ = sqlx::query("SELECT 1").fetch_one(&mut conn).await.unwrap();

                // conn gets dropped here and should be returned to the pool
            }

            // (do some other work here without holding on to a connection)
            // this actually fixes the issue, depending on the timeout used
            // sqlx_rt::sleep(Duration::from_millis(500)).await;

            {
                let start = Instant::now();
                match pool.acquire().await {
                    Ok(conn) => {
                        println!("{} acquire took {:?}", i, start.elapsed());
                        drop(conn);
                    }
                    Err(e) => panic!("{} acquire returned error: {} pool state: {:?}", i, e, pool),
                }
            }

            Result::<(), anyhow::Error>::Ok(())
        }));
    }

    futures::future::try_join_all(handles).await?;

    Ok(())
}

#[sqlx_macros::test]
async fn test_describe_outer_join_nullable() -> anyhow::Result<()> {
    let mut conn = new::<Postgres>().await?;

    // test nullability inference for various joins

    // inner join, nullability should not be overridden
    // language=PostgreSQL
    let describe = conn
        .describe(
            "select tweet.id
from (values (null)) vals(val)
         inner join tweet on false",
        )
        .await?;

    assert_eq!(describe.nullable(0), Some(false));

    // language=PostgreSQL
    let describe = conn
        .describe(
            "select tweet.id
from (values (null)) vals(val)
         left join tweet on false",
        )
        .await?;

    // tweet.id is marked NOT NULL but it's brought in from a left-join here
    // which should make it nullable
    assert_eq!(describe.nullable(0), Some(true));

    // make sure we don't mis-infer for the outer half of the join
    // language=PostgreSQL
    let describe = conn
        .describe(
            "select tweet1.id, tweet2.id
    from tweet tweet1
    left join tweet tweet2 on false",
        )
        .await?;

    assert_eq!(describe.nullable(0), Some(false));
    assert_eq!(describe.nullable(1), Some(true));

    // right join, nullability should be inverted
    // language=PostgreSQL
    let describe = conn
        .describe(
            "select tweet1.id, tweet2.id
    from tweet tweet1
    right join tweet tweet2 on false",
        )
        .await?;

    assert_eq!(describe.nullable(0), Some(true));
    assert_eq!(describe.nullable(1), Some(false));

    // full outer join, both tables are nullable
    // language=PostgreSQL
    let describe = conn
        .describe(
            "select tweet1.id, tweet2.id
    from tweet tweet1
    full join tweet tweet2 on false",
        )
        .await?;

    assert_eq!(describe.nullable(0), Some(true));
    assert_eq!(describe.nullable(1), Some(true));

    Ok(())
}

#[sqlx_macros::test]
async fn test_listener_cleanup() -> anyhow::Result<()> {
    #[cfg(any(feature = "_rt-tokio", feature = "_rt-actix"))]
    use tokio::time::timeout;

    #[cfg(feature = "_rt-async-std")]
    use async_std::future::timeout;

    use sqlx::pool::PoolOptions;
    use sqlx::postgres::PgListener;

    // Create a connection on which to send notifications
    let mut notify_conn = new::<Postgres>().await?;

    // Create a pool with exactly one connection so we can
    // deterministically test the cleanup.
    let pool = PoolOptions::<Postgres>::new()
        .min_connections(1)
        .max_connections(1)
        .test_before_acquire(true)
        .connect(&env::var("DATABASE_URL")?)
        .await?;

    let mut listener = PgListener::connect_with(&pool).await?;
    listener.listen("test_channel").await?;

    // Checks for a notification on the test channel
    async fn try_recv(listener: &mut PgListener) -> anyhow::Result<bool> {
        match timeout(Duration::from_millis(100), listener.recv()).await {
            Ok(res) => {
                res?;
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }

    // Check no notification is received before one is sent
    assert!(!try_recv(&mut listener).await?, "Notification not sent");

    // Check notification is sent and received
    notify_conn.execute("NOTIFY test_channel").await?;
    assert!(
        try_recv(&mut listener).await?,
        "Notification sent and received"
    );
    assert!(
        !try_recv(&mut listener).await?,
        "Notification is not duplicated"
    );

    // Test that cleanup stops listening on the channel
    drop(listener);
    let mut listener = PgListener::connect_with(&pool).await?;

    // Check notification is sent but not received
    notify_conn.execute("NOTIFY test_channel").await?;
    assert!(
        !try_recv(&mut listener).await?,
        "Notification is not received on fresh listener"
    );

    Ok(())
}

#[sqlx_macros::test]
async fn it_supports_domain_types_in_composite_domain_types() -> anyhow::Result<()> {
    #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    struct MonthId(i16);

    impl sqlx::Type<Postgres> for MonthId {
        fn type_info() -> sqlx::postgres::PgTypeInfo {
            sqlx::postgres::PgTypeInfo::with_name("month_id")
        }

        fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
            *ty == Self::type_info()
        }
    }

    impl<'r> sqlx::Decode<'r, Postgres> for MonthId {
        fn decode(
            value: sqlx::postgres::PgValueRef<'r>,
        ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
            Ok(Self(<i16 as sqlx::Decode<Postgres>>::decode(value)?))
        }
    }

    impl<'q> sqlx::Encode<'q, Postgres> for MonthId {
        fn encode_by_ref(
            &self,
            buf: &mut sqlx::postgres::PgArgumentBuffer,
        ) -> sqlx::encode::IsNull {
            self.0.encode(buf)
        }
    }

    #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    struct WinterYearMonth {
        year: i32,
        month: MonthId,
    }

    impl sqlx::Type<Postgres> for WinterYearMonth {
        fn type_info() -> sqlx::postgres::PgTypeInfo {
            sqlx::postgres::PgTypeInfo::with_name("winter_year_month")
        }

        fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
            *ty == Self::type_info()
        }
    }

    impl<'r> sqlx::Decode<'r, Postgres> for WinterYearMonth {
        fn decode(
            value: sqlx::postgres::PgValueRef<'r>,
        ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
            let mut decoder = sqlx::postgres::types::PgRecordDecoder::new(value)?;

            let year = decoder.try_decode::<i32>()?;
            let month = decoder.try_decode::<MonthId>()?;

            Ok(Self { year, month })
        }
    }

    impl<'q> sqlx::Encode<'q, Postgres> for WinterYearMonth {
        fn encode_by_ref(
            &self,
            buf: &mut sqlx::postgres::PgArgumentBuffer,
        ) -> sqlx::encode::IsNull {
            let mut encoder = sqlx::postgres::types::PgRecordEncoder::new(buf);
            encoder.encode(self.year);
            encoder.encode(self.month);
            encoder.finish();
            sqlx::encode::IsNull::No
        }
    }

    let mut conn = new::<Postgres>().await?;

    {
        let result = sqlx::query("DELETE FROM heating_bills;")
            .execute(&mut conn)
            .await;

        let result = result.unwrap();
        assert_eq!(result.rows_affected(), 1);
    }

    {
        let result = sqlx::query(
            "INSERT INTO heating_bills(month, cost) VALUES($1::winter_year_month, 100);",
        )
        .bind(WinterYearMonth {
            year: 2021,
            month: MonthId(1),
        })
        .execute(&mut conn)
        .await;

        let result = result.unwrap();
        assert_eq!(result.rows_affected(), 1);
    }

    {
        let result = sqlx::query("DELETE FROM heating_bills;")
            .execute(&mut conn)
            .await;

        let result = result.unwrap();
        assert_eq!(result.rows_affected(), 1);
    }

    Ok(())
}
