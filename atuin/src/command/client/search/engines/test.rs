use atuin_client::{database::{Database, Context}, history::History, settings::FilterMode};
use chrono::{Utc, TimeZone};

use super::{SearchEngine, SearchState};

async fn entries(since: chrono::DateTime<chrono::Utc>) -> atuin_client::database::Sqlite {
    use atuin_client::database::Sqlite;
    use chrono::Duration;

    let mut db = Sqlite::new("sqlite::memory:").await.unwrap();

    let mut history: [History; 2] = [
        History::import()
            .timestamp(since - Duration::days(2))
            .command("docker run -e POSTGRES_USER=atuin -e POSTGRES_PASSWORD=pass -e POSTGRES_DB=atuin -p 5432:5432 -d --rm postgres:14-alpine")
            .session("1")
            .cwd("/Users/conrad/code/atuin")
            .hostname("host1:conrad")
            .build()
            .into(),
        History::import()
            .timestamp(since - Duration::days(2))
            .command("cargo sqlx prepare --database-url postgresql://user:pass@localhost/store")
            .session("1")
            .cwd("/Users/conrad/code/atuin")
            .hostname("host1:conrad")
            .build()
            .into(),
    ];

    for (i, h) in history.iter_mut().enumerate() {
        h.id = i.to_string();
    }

    db.save_bulk(&history).await.unwrap();

    db
}

pub async fn docker_postgres(mut search: impl SearchEngine) -> Vec<History> {
    let now = Utc.ymd(2023, 6, 18).and_hms(11, 14, 16);
    let mut db = entries(now).await;

    let state = SearchState {
        input: "docker postgres".to_owned().into(),
        filter_mode: FilterMode::Global,
        context: Context {
            session: "1".into(),
            cwd: "/Users/conrad/code/atuin".into(),
            hostname: "host1:conrad".into(),
            host_id: String::new(),
        },
    };

    search.full_query(&state, &mut db).await.unwrap()
}

pub async fn postgres(mut search: impl SearchEngine) -> Vec<History> {
    let now = Utc.ymd(2023, 6, 18).and_hms(11, 14, 16);
    let mut db = entries(now).await;

    let state = SearchState {
        input: "postgres".to_owned().into(),
        filter_mode: FilterMode::Global,
        context: Context {
            session: "1".into(),
            cwd: "/Users/conrad/code/atuin".into(),
            hostname: "host1:conrad".into(),
            host_id: String::new(),
        },
    };

    search.full_query(&state, &mut db).await.unwrap()
}
