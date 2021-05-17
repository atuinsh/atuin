use sqlx::migrate::Migrator;
use std::path::Path;

static EMBEDDED: Migrator = sqlx::migrate!("tests/migrate/migrations");

#[sqlx_macros::test]
async fn same_output() -> anyhow::Result<()> {
    let runtime = Migrator::new(Path::new("tests/migrate/migrations")).await?;

    for (e, r) in EMBEDDED.iter().zip(runtime.iter()) {
        assert_eq!(e.version, r.version);
        assert_eq!(e.description, r.description);
        assert_eq!(e.sql, r.sql);
        assert_eq!(e.checksum, r.checksum);
    }

    Ok(())
}
