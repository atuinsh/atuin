use sqlx::any::AnyPoolOptions;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

#[sqlx_macros::test]
async fn pool_should_invoke_after_connect() -> anyhow::Result<()> {
    let counter = Arc::new(AtomicUsize::new(0));

    let pool = AnyPoolOptions::new()
        .after_connect({
            let counter = counter.clone();
            move |_conn| {
                let counter = counter.clone();
                Box::pin(async move {
                    counter.fetch_add(1, Ordering::SeqCst);

                    Ok(())
                })
            }
        })
        .connect(&dotenv::var("DATABASE_URL")?)
        .await?;

    let _ = pool.acquire().await?;
    let _ = pool.acquire().await?;
    let _ = pool.acquire().await?;
    let _ = pool.acquire().await?;

    // since connections are released asynchronously,
    // `.after_connect()` may be called more than once
    assert!(counter.load(Ordering::SeqCst) >= 1);

    Ok(())
}

// https://github.com/launchbadge/sqlx/issues/527
#[sqlx_macros::test]
async fn pool_should_be_returned_failed_transactions() -> anyhow::Result<()> {
    let pool = AnyPoolOptions::new()
        .max_connections(2)
        .connect_timeout(Duration::from_secs(3))
        .connect(&dotenv::var("DATABASE_URL")?)
        .await?;

    let query = "blah blah";

    let mut tx = pool.begin().await?;
    let res = sqlx::query(query).execute(&mut tx).await;
    assert!(res.is_err());
    drop(tx);

    let mut tx = pool.begin().await?;
    let res = sqlx::query(query).execute(&mut tx).await;
    assert!(res.is_err());
    drop(tx);

    let mut tx = pool.begin().await?;
    let res = sqlx::query(query).execute(&mut tx).await;
    assert!(res.is_err());
    drop(tx);

    Ok(())
}
