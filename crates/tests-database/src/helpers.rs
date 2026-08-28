use std::env::{self, temp_dir};

use atuin_server_database::{DbSettings, DbType};
use snowflake_uid::{Config, Generator};
use sqlx::migrate::MigrateDatabase;
use url::Url;

fn get_settings(env_uri: Option<String>) -> eyre::Result<DbSettings> {
    let db_uri = env_uri.unwrap_or_else(|| {
        let dir = temp_dir();
        let file = dir.join("atuin_test_db_");
        let filename = file.to_str().unwrap();
        format!("sqlite://{filename}")
    });

    let mut url = Url::parse(&db_uri)?;
    let cfg = Config::default();
    let mut generator = Generator::from(cfg, 0);
    let snowflake = generator.get();

    let unique_path = format!("{}{snowflake}", url.path());
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
        DbType::MySql => sqlx::MySql::create_database(&settings.db_uri).await?,
        DbType::Unknown => todo!(),
    };

    Ok(settings)
}

#[allow(dead_code)]
pub async fn destroy_test_db(settings: &DbSettings) -> eyre::Result<()> {
    match settings.db_type() {
        DbType::Postgres => sqlx::Postgres::drop_database(&settings.db_uri).await?,
        DbType::Sqlite => sqlx::Sqlite::drop_database(&settings.db_uri).await?,
        DbType::MySql => sqlx::MySql::drop_database(&settings.db_uri).await?,
        DbType::Unknown => todo!(),
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use regex::Regex;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::none(None, r"sqlite://.*[\\/]atuin_test_db_\d+")]
    #[case::with_param(
        Some("postgres://user:pass@host/database_?mode=ssl".into()),
        r"postgres://user:pass@host/database_\d+\?mode=ssl"
    )]
    fn settings(#[case] input: Option<String>, #[case] pattern: &str) -> eyre::Result<()> {
        let settings = get_settings(input)?;
        let re = Regex::new(pattern)?;
        assert!(re.is_match(&settings.db_uri), "{}", &settings.db_uri);
        Ok(())
    }
}
