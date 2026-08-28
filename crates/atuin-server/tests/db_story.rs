use std::env::{self, temp_dir};

use atuin_common::db::DbUrl;
use atuin_common::utils::{crypto_random_string, uuid_v7};
use atuin_domain::record::{
    EncryptedData, Host, HostId, Record, RecordIdx, RecordSeriesKey, RecordTag,
};
use atuin_server::db::models::{NewSession, NewUser, User};
use atuin_server::db::{Database, DbError, DbSettings, MySql, Postgres, Sqlite};
use rstest::rstest;
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
    let unique = uuid_v7().as_simple().to_string();

    let unique_path = format!("{}{unique}", url.path());
    url.set_path(&unique_path);

    let db_uri = url.to_string();

    Ok(DbSettings {
        db_uri: db_uri.parse()?,
    })
}

async fn create_test_db() -> eyre::Result<DbSettings> {
    let var = env::var("ATUIN_TEST_DB_URI").ok();
    let settings = get_settings(var)?;

    match &settings.db_uri {
        DbUrl::Postgres(_) => sqlx::Postgres::create_database(settings.db_uri.as_str()).await?,
        DbUrl::Sqlite(_) => sqlx::Sqlite::create_database(settings.db_uri.as_str()).await?,
        DbUrl::Mysql(_) => sqlx::MySql::create_database(settings.db_uri.as_str()).await?,
    };

    Ok(settings)
}

async fn destroy_test_db(settings: &DbSettings) -> eyre::Result<()> {
    match &settings.db_uri {
        DbUrl::Postgres(_) => sqlx::Postgres::drop_database(settings.db_uri.as_str()).await?,
        DbUrl::Sqlite(_) => sqlx::Sqlite::drop_database(settings.db_uri.as_str()).await?,
        DbUrl::Mysql(_) => sqlx::MySql::drop_database(settings.db_uri.as_str()).await?,
    };
    Ok(())
}

struct TestDb {
    settings: DbSettings,
}

impl TestDb {
    async fn new() -> eyre::Result<Self> {
        let settings = create_test_db().await?;
        Ok(Self { settings })
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let settings = self.settings.clone();
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                if let Err(e) = destroy_test_db(&settings).await {
                    eprintln!("Failed to destroy test db: {e:?}");
                }
            });
        })
        .join();
    }
}

/// This test runs through a story of using the database. The goal is to fully exercise all DB code
/// in a single repeatable manner.
#[rstest]
#[tokio::test]
async fn test_full_db_story() -> eyre::Result<()> {
    let test_db = TestDb::new().await?;
    let settings = &test_db.settings;

    match &settings.db_uri {
        DbUrl::Postgres(url) => run_the_test::<Postgres>(url.clone()).await,
        DbUrl::Sqlite(url) => run_the_test::<Sqlite>(url.clone()).await,
        DbUrl::Mysql(url) => run_the_test::<MySql>(url.clone()).await,
    }
}

async fn run_the_test<DB: Database>(url: DB::Url) -> eyre::Result<()> {
    let db = DB::connect(url).await?;
    // register a user
    let new_user = NewUser {
        username: "foo".to_owned(),
        email: "foo@example.com".to_owned(),
        password: "hunter2".to_owned(),
    };
    let user_id = db.add_user(&new_user).await?;
    assert_ne!(user_id, 0);

    let token = crypto_random_string::<24>();
    let new_session = NewSession {
        user_id,
        token: token.clone(),
    };
    db.add_session(&new_session).await?;

    // The user is now registered and has a session. This happens when a user logs in
    let user = db.get_session_user(&token).await?;
    assert_eq!(user.username, "foo");

    let session = db.get_session(&token).await?;
    assert_eq!(session.user_id, user_id);

    let user = db.get_user("foo").await?;
    assert_eq!(user.password, "hunter2");

    // Lets change the password
    let user = User {
        email: "foo@example.com".to_owned(),
        id: user_id,
        password: "hunter3".to_owned(),
        username: "foo".to_owned(),
    };
    db.update_user_password(&user).await?;

    let user = db.get_user("foo").await?;
    assert_eq!(user.password, "hunter3");

    // add a bunch of records
    let host_a = Host::new(HostId(uuid_v7()));
    let host_b = Host::new(HostId(uuid_v7()));
    let records = vec![
        generate_record(&host_a, 1),
        generate_record(&host_b, 2),
        generate_record(&host_a, 2),
        generate_record(&host_b, 2),
        generate_record(&host_a, 3),
        generate_record(&host_a, 4),
        generate_record(&host_a, 5),
        generate_record(&host_a, 6),
    ];
    db.add_records(&user, &records).await?;

    let status = db.status(&user).await?;
    assert!(status.hosts.contains_key(&host_a.id));
    assert!(status.hosts.contains_key(&host_b.id));
    assert_eq!(status.hosts.get(&host_a.id).unwrap().get(&RecordTag::History).unwrap().clone(), 6);
    assert_eq!(status.hosts.get(&host_b.id).unwrap().get(&RecordTag::History).unwrap().clone(), 2);

    // Get 3 records from the beginning
    let recs = db
        .next_records(&user, &RecordSeriesKey::new(host_a.id, RecordTag::History), None, 3)
        .await?;
    assert_eq!(recs.len(), 3);
    assert_eq!(recs[0].idx, 1);
    assert_eq!(recs.last().unwrap().idx, 3);

    // Get from the end, for host a. Get more than exists
    let recs = db
        .next_records(&user, &RecordSeriesKey::new(host_a.id, RecordTag::History), Some(4), 10)
        .await?;
    assert_eq!(recs.len(), 3);
    assert_eq!(recs[0].idx, 4); // check the head record is idx 4
    assert_eq!(recs.last().unwrap().idx, 6);

    // delete_store
    db.delete_store(&user).await?;
    let recs = db
        .next_records(&user, &RecordSeriesKey::new(host_a.id, RecordTag::History), Some(4), 10)
        .await?;
    assert_eq!(recs.len(), 0);

    // Converged behavior: delete_user must purge the user's store rows on every
    // backend (SQLite previously left them behind).
    db.add_records(&user, &records).await?;
    db.delete_user(&user).await?;

    let recs = db
        .next_records(&user, &RecordSeriesKey::new(host_a.id, RecordTag::History), None, 10)
        .await?;
    assert_eq!(recs.len(), 0, "delete_user must purge store rows");

    let missing = db.get_user("foo").await;
    assert!(matches!(missing, Err(DbError::NotFound)), "user should be gone after delete_user");

    Ok(())
}

fn generate_record(host: &Host, idx: RecordIdx) -> Record<EncryptedData> {
    let data = EncryptedData {
        raw: "some data".into(),
        cek: "key".into(),
    };
    Record::builder()
        .idx(idx)
        .host(host.clone())
        .version("2".into())
        .tag(RecordTag::History)
        .data(data)
        .build()
}

#[cfg(test)]
mod tests {
    use regex::Regex;
    use rstest::rstest;

    use super::get_settings;

    #[rstest]
    #[case::none(None, r"sqlite://.*[\\/]atuin_test_db_[0-9a-f]+")]
    #[case::with_param(
        Some("postgres://user:pass@host/database_?mode=ssl".into()),
        r"postgres://user:pass@host/database_[0-9a-f]+\?mode=ssl"
    )]
    fn settings(#[case] input: Option<String>, #[case] pattern: &str) -> eyre::Result<()> {
        let settings = get_settings(input)?;
        let re = Regex::new(pattern)?;
        assert!(re.is_match(settings.db_uri.as_str()), "{}", settings.db_uri.as_str());
        Ok(())
    }
}
