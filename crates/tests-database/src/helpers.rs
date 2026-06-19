use std::env::{self, temp_dir};

use atuin_server_database::{DbSettings, DbType};
use sqlx::migrate::MigrateDatabase;
use url::Url;
use uuid::Uuid;

fn get_settings(env_uri: Option<String>) -> eyre::Result<DbSettings> {
    let db_uri = env_uri.unwrap_or_else(|| {
        let dir = temp_dir();
        let file = dir.join("atuin_test_db_");
        let filename = file.to_str().unwrap();
        format!("sqlite://{filename}")
    });

    let mut url = Url::parse(&db_uri)?;

    // Append a random UUID so every call gets a distinct database, even when
    // called concurrently within the same millisecond. (A per-call snowflake
    // generator re-seeded from scratch could emit duplicate ids under that
    // pattern, causing parallel tests to collide on the same SQLite file.)
    let unique = Uuid::new_v4().simple();
    let unique_path = format!("{}{unique}", url.path());
    url.set_path(&unique_path);

    let db_uri = url.to_string();

    Ok(DbSettings {
        db_uri,
        read_db_uri: None,
    })
}

#[allow(dead_code)]
pub async fn create_test_db() -> eyre::Result<DbSettings> {
    let var = env::var("ATUIN_TEST_DB_URI").ok();
    let settings = get_settings(var)?;

    match settings.db_type() {
        DbType::Postgres => sqlx::Postgres::create_database(&settings.db_uri).await?,
        DbType::Sqlite => sqlx::Sqlite::create_database(&settings.db_uri).await?,
        atuin_server_database::DbType::Unknown => todo!(),
    };

    Ok(settings)
}

#[allow(dead_code)]
pub async fn destroy_test_db(settings: &DbSettings) -> eyre::Result<()> {
    match settings.db_type() {
        DbType::Postgres => sqlx::Postgres::drop_database(&settings.db_uri).await?,
        DbType::Sqlite => sqlx::Sqlite::drop_database(&settings.db_uri).await?,
        DbType::Unknown => todo!(),
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    #[test]
    fn test_settings_none() -> eyre::Result<()> {
        let settings = get_settings(None)?;
        let re = Regex::new(r"sqlite://.*[\\/]atuin_test_db_[0-9a-f]+").unwrap();
        assert!(re.is_match(&settings.db_uri), "{}", &settings.db_uri);
        Ok(())
    }

    #[test]
    fn test_settings_with_param() -> eyre::Result<()> {
        let settings = get_settings(Some("postgres://user:pass@host/database_?mode=ssl".into()))?;
        let re = Regex::new(r"postgres://user:pass@host/database_[0-9a-f]+\?mode=ssl")?;
        assert!(re.is_match(&settings.db_uri), "{}", &settings.db_uri);

        Ok(())
    }

    // Regression: get_settings must produce a unique DB path on every call, even
    // when called in a tight burst (within the same millisecond). The previous
    // snowflake-based suffix re-seeded a fresh generator per call and could emit
    // duplicate ids under that pattern, which made parallel tests collide on the
    // same SQLite file ("table already exists" during migration).
    #[test]
    fn test_settings_paths_are_unique_in_a_burst() -> eyre::Result<()> {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            let settings = get_settings(None)?;
            assert!(
                seen.insert(settings.db_uri.clone()),
                "duplicate db_uri generated: {}",
                settings.db_uri
            );
        }
        Ok(())
    }
}
