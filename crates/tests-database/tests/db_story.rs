use atuin_common::{
    record::{EncryptedData, Host, HostId, Record, RecordIdx},
    utils::{crypto_random_string, uuid_v7},
};
use atuin_server_database::{
    Database, DbSettings, DbType,
    models::{NewHistory, NewSession, NewUser, User},
};
use atuin_server_mysql::MySql;
use atuin_server_postgres::Postgres;
use atuin_server_sqlite::Sqlite;
use tests_database::helpers::{create_test_db, destroy_test_db};
use time::OffsetDateTime;
use uuid::Uuid;

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
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                if let Err(e) = destroy_test_db(&settings).await {
                    eprintln!("Failed to destroy test db: {:?}", e);
                }
            });
        })
        .join();
    }
}

/// This test runs through a story of using the database. The goal is to fully exercise all DB code
/// in a single repeatable manner.
#[tokio::test]
async fn test_full_db_story() -> eyre::Result<()> {
    let test_db = TestDb::new().await?;
    let settings = &test_db.settings;

    match settings.db_type() {
        DbType::Postgres => run_the_test::<Postgres>(settings).await,
        DbType::Sqlite => run_the_test::<Sqlite>(settings).await,
        DbType::MySql => run_the_test::<MySql>(settings).await,
        DbType::Unknown => todo!(),
    }
}

async fn run_the_test<DB: Database>(settings: &DbSettings) -> eyre::Result<()> {
    let db = DB::new(settings).await?;
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

    // add some history
    let h = vec![
        generate_history(user_id),
        generate_history(user_id),
        generate_history(user_id),
        generate_history(user_id),
    ];
    db.add_history(&h).await?;

    assert_eq!(db.count_history(&user).await?, 4);

    // AFAICT history is not used any more so I'm not going figure out how to take the timestamps
    // from generated history into this
    // assert_eq!(db.count_history_range(&user).await?, 4);

    db.delete_history(&user, h[0].client_id.clone()).await?;
    let deleted_history = db.deleted_history(&user).await?;
    assert_eq!(deleted_history.len(), 1);

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
    assert_eq!(
        status
            .hosts
            .get(&host_a.id)
            .unwrap()
            .get("history")
            .unwrap()
            .clone(),
        6
    );
    assert_eq!(
        status
            .hosts
            .get(&host_b.id)
            .unwrap()
            .get("history")
            .unwrap()
            .clone(),
        2
    );

    // Get 3 records from the beginning
    let recs = db
        .next_records(&user, host_a.id, "history".into(), None, 3)
        .await?;
    assert_eq!(recs.len(), 3);
    assert_eq!(recs[0].idx, 1);
    assert_eq!(recs.last().unwrap().idx, 3);

    // Get from the end, for host a. Get more than exists
    let recs = db
        .next_records(&user, host_a.id, "history".into(), Some(4), 10)
        .await?;
    assert_eq!(recs.len(), 3);
    assert_eq!(recs[0].idx, 4); // check the head record is idx 4
    assert_eq!(recs.last().unwrap().idx, 6);

    // delete_store
    db.delete_store(&user).await?;
    let recs = db
        .next_records(&user, host_a.id, "history".into(), Some(4), 10)
        .await?;
    assert_eq!(recs.len(), 0);

    Ok(())
}

fn generate_history(user_id: i64) -> NewHistory {
    use fake::Fake;
    use fake::faker::lorem::en::*;

    let data: String = Sentence(1..3).fake();
    let hostname: String = "foo".to_owned();
    let client_id = Uuid::new_v4().to_string();

    let timestamp: OffsetDateTime = OffsetDateTime::now_utc();

    NewHistory {
        client_id,
        user_id,
        hostname,
        timestamp,
        data,
    }
}

fn generate_record(host: &Host, idx: RecordIdx) -> Record<EncryptedData> {
    let data = EncryptedData {
        data: "some data".into(),
        content_encryption_key: "key".into(),
    };
    Record::builder()
        .idx(idx)
        .host(host.clone())
        .version("2".into())
        .tag("history".into())
        .data(data)
        .build()
}
