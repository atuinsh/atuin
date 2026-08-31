//! Regression test for D4: orphaned `store` rows must be purged on Postgres.
//!
//! The historical `delete_user` bug lived in the SHARED blanket-impl path that
//! serves both SQLite and Postgres, so Postgres deployments that ran the buggy
//! code carry orphaned `store` rows (rows whose `user_id` no longer exists in
//! `users`). SQLite shipped a cleanup migration; Postgres did not. This test
//! proves the Postgres migration chain purges those orphans.
//!
//! It only runs when `ATUIN_TEST_DB_URI` points at a Postgres instance, e.g.:
//!
//!   docker run -d --rm --name d4-pg -e POSTGRES_USER=atuin \
//!     -e POSTGRES_PASSWORD=pass -e POSTGRES_DB=atuin -p 15439:5432 postgres:16
//!   ATUIN_TEST_DB_URI=postgres://atuin:pass@localhost:15439/atuin \
//!     cargo test -p atuin-server --test pg_purge_orphaned_store

use sqlx::{Row, postgres::PgPoolOptions};

#[tokio::test]
async fn postgres_migration_purges_orphaned_store_rows() -> eyre::Result<()> {
    let Ok(uri) = std::env::var("ATUIN_TEST_DB_URI") else {
        eprintln!("skipping: ATUIN_TEST_DB_URI not set");
        return Ok(());
    };
    if !uri.starts_with("postgres") {
        eprintln!("skipping: ATUIN_TEST_DB_URI is not a Postgres URI");
        return Ok(());
    }

    let pool = PgPoolOptions::new().max_connections(4).connect(&uri).await?;

    // Bring the schema up to head using the exact embedded Postgres migration
    // chain the server ships.
    let migrator = sqlx::migrate!("src/db/postgres/migrations");
    migrator.run(&pool).await?;

    // Simulate a database that ran the old buggy code: two real users with
    // owned store rows, plus orphaned store rows pointing at users that no
    // longer exist.
    sqlx::query("delete from store").execute(&pool).await?;
    sqlx::query("delete from users").execute(&pool).await?;

    let alice: i64 = sqlx::query(
        "insert into users (username, email, password) values ('alice', 'a@example.com', 'pw-a') \
         returning id",
    )
    .fetch_one(&pool)
    .await?
    .get(0);
    let bob: i64 = sqlx::query(
        "insert into users (username, email, password) values ('bob', 'b@example.com', 'pw-b') \
         returning id",
    )
    .fetch_one(&pool)
    .await?
    .get(0);

    // A user_id guaranteed not to exist in `users`.
    let orphan_uid = alice.max(bob) + 1000;

    let insert_store = |uid: i64| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "insert into store (id, client_id, host, idx, timestamp, version, tag, data, cek, \
                 user_id) values (gen_random_uuid(), gen_random_uuid(), gen_random_uuid(), 1, 0, \
                 '1', 'history', 'd', 'k', $1)",
            )
            .bind(uid)
            .execute(&pool)
            .await
        }
    };

    // Owned rows (must survive) and orphaned rows (must be purged).
    insert_store(alice).await?;
    insert_store(bob).await?;
    insert_store(orphan_uid).await?;
    insert_store(orphan_uid).await?;

    // Sanity: the orphans are present before we apply the purge.
    let orphans_before: i64 =
        sqlx::query("select count(*) from store where user_id not in (select id from users)")
            .fetch_one(&pool)
            .await?
            .get(0);
    assert_eq!(orphans_before, 2, "test setup should have created 2 orphaned store rows");

    // Apply the purge migration from the embedded Postgres chain. With the fix
    // present, this migration exists and removes the orphans; without it, no
    // purge runs and the orphans survive, failing the assertion below.
    if let Some(purge) =
        migrator.iter().find(|m| m.description.contains("purge-orphaned-store"))
    {
        sqlx::query(sqlx::AssertSqlSafe(purge.sql.as_str().to_owned())).execute(&pool).await?;
    }

    let orphans_after: i64 =
        sqlx::query("select count(*) from store where user_id not in (select id from users)")
            .fetch_one(&pool)
            .await?
            .get(0);
    assert_eq!(
        orphans_after, 0,
        "orphaned store rows must be purged by the Postgres migration chain"
    );

    // Owned rows must be untouched.
    let owned: i64 = sqlx::query("select count(*) from store").fetch_one(&pool).await?.get(0);
    assert_eq!(owned, 2, "store rows owned by existing users must be retained");

    Ok(())
}
