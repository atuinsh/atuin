use std::{path::Path, str::FromStr, time::Duration};

use atuin_common::utils;
use sqlx::{
    Result, Row,
    sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow,
        SqliteSynchronous,
    },
};
use tokio::fs;
use tracing::debug;
use uuid::Uuid;

use crate::store::script::Script;

#[derive(Debug, Clone)]
pub struct Database {
    pub pool: SqlitePool,
}

impl Database {
    pub async fn new(path: impl AsRef<Path>, timeout: f64) -> Result<Self> {
        let path = path.as_ref();
        debug!("opening script sqlite database at {:?}", path);

        if utils::broken_symlink(path) {
            eprintln!(
                "Atuin: Script sqlite db path ({path:?}) is a broken symlink. Unable to read or create replacement."
            );
            std::process::exit(1);
        }

        if !path.exists()
            && let Some(dir) = path.parent()
        {
            fs::create_dir_all(dir).await?;
        }

        let opts = SqliteConnectOptions::from_str(path.as_os_str().to_str().unwrap())?
            .journal_mode(SqliteJournalMode::Wal)
            .optimize_on_close(true, None)
            .synchronous(SqliteSynchronous::Normal)
            .with_regexp()
            .foreign_keys(true)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .acquire_timeout(Duration::from_secs_f64(timeout))
            .connect_with(opts)
            .await?;

        Self::setup_db(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn sqlite_version(&self) -> Result<String> {
        sqlx::query_scalar("SELECT sqlite_version()")
            .fetch_one(&self.pool)
            .await
    }

    async fn setup_db(pool: &SqlitePool) -> Result<()> {
        debug!("running sqlite database setup");

        sqlx::migrate!("./migrations").run(pool).await?;

        Ok(())
    }

    async fn save_raw(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, s: &Script) -> Result<()> {
        sqlx::query(
            "insert or ignore into scripts(id, name, description, shebang, script)
                values(?1, ?2, ?3, ?4, ?5)",
        )
        .bind(s.id.to_string())
        .bind(s.name.as_str())
        .bind(s.description.as_str())
        .bind(s.shebang.as_str())
        .bind(s.script.as_str())
        .execute(&mut **tx)
        .await?;

        for tag in s.tags.iter() {
            sqlx::query(
                "insert or ignore into script_tags(script_id, tag)
                values(?1, ?2)",
            )
            .bind(s.id.to_string())
            .bind(tag)
            .execute(&mut **tx)
            .await?;
        }

        Ok(())
    }

    pub async fn save(&self, s: &Script) -> Result<()> {
        debug!("saving script to sqlite");
        let mut tx = self.pool.begin().await?;
        Self::save_raw(&mut tx, s).await?;
        tx.commit().await?;

        Ok(())
    }

    pub async fn save_bulk(&self, s: &[Script]) -> Result<()> {
        debug!("saving scripts to sqlite");

        let mut tx = self.pool.begin().await?;

        for i in s {
            Self::save_raw(&mut tx, i).await?;
        }

        tx.commit().await?;

        Ok(())
    }

    fn query_script(row: SqliteRow) -> Script {
        let id = row.get("id");
        let name = row.get("name");
        let description = row.get("description");
        let shebang = row.get("shebang");
        let script = row.get("script");

        let id = Uuid::parse_str(id).unwrap();

        Script {
            id,
            name,
            description,
            shebang,
            script,
            tags: vec![],
        }
    }

    fn query_script_tags(row: SqliteRow) -> String {
        row.get("tag")
    }

    #[allow(dead_code)]
    async fn load(&self, id: &str) -> Result<Option<Script>> {
        debug!("loading script item {}", id);

        let res = sqlx::query("select * from scripts where id = ?1")
            .bind(id)
            .map(Self::query_script)
            .fetch_optional(&self.pool)
            .await?;

        // intentionally not joining, don't want to duplicate the script data in memory a whole bunch.
        if let Some(mut script) = res {
            let tags = sqlx::query("select tag from script_tags where script_id = ?1")
                .bind(id)
                .map(Self::query_script_tags)
                .fetch_all(&self.pool)
                .await?;

            script.tags = tags;
            Ok(Some(script))
        } else {
            Ok(None)
        }
    }

    pub async fn list(&self) -> Result<Vec<Script>> {
        debug!("listing scripts");

        let mut res = sqlx::query("select * from scripts")
            .map(Self::query_script)
            .fetch_all(&self.pool)
            .await?;

        // Fetch all the tags for each script
        for script in res.iter_mut() {
            let tags = sqlx::query("select tag from script_tags where script_id = ?1")
                .bind(script.id.to_string())
                .map(Self::query_script_tags)
                .fetch_all(&self.pool)
                .await?;

            script.tags = tags;
        }

        Ok(res)
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        debug!("deleting script {}", id);

        sqlx::query("delete from scripts where id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await?;

        // delete all the tags for the script
        sqlx::query("delete from script_tags where script_id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn update(&self, s: &Script) -> Result<()> {
        debug!("updating script {:?}", s);

        let mut tx = self.pool.begin().await?;

        // Update the script's base fields
        sqlx::query("update scripts set name = ?1, description = ?2, shebang = ?3, script = ?4 where id = ?5")
            .bind(s.name.as_str())
            .bind(s.description.as_str())
            .bind(s.shebang.as_str())
            .bind(s.script.as_str())
            .bind(s.id.to_string())
            .execute(&mut *tx)
            .await?;

        // Delete all existing tags for this script
        sqlx::query("delete from script_tags where script_id = ?1")
            .bind(s.id.to_string())
            .execute(&mut *tx)
            .await?;

        // Insert new tags
        for tag in s.tags.iter() {
            sqlx::query(
                "insert or ignore into script_tags(script_id, tag)
                values(?1, ?2)",
            )
            .bind(s.id.to_string())
            .bind(tag)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;

        Ok(())
    }

    pub async fn get_by_name(&self, name: &str) -> Result<Option<Script>> {
        let res = sqlx::query("select * from scripts where name = ?1")
            .bind(name)
            .map(Self::query_script)
            .fetch_optional(&self.pool)
            .await?;

        let script = if let Some(mut script) = res {
            let tags = sqlx::query("select tag from script_tags where script_id = ?1")
                .bind(script.id.to_string())
                .map(Self::query_script_tags)
                .fetch_all(&self.pool)
                .await?;

            script.tags = tags;
            Some(script)
        } else {
            None
        };

        Ok(script)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn test_list() {
        let db = Database::new("sqlite::memory:", 1.0).await.unwrap();
        let scripts = db.list().await.unwrap();
        assert_eq!(scripts.len(), 0);

        let script = Script::builder()
            .name("test".to_string())
            .description("test".to_string())
            .shebang("test".to_string())
            .script("test".to_string())
            .build();

        db.save(&script).await.unwrap();

        let scripts = db.list().await.unwrap();
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].name, "test");
    }

    #[tokio::test]
    async fn test_save_load() {
        let db = Database::new("sqlite::memory:", 1.0).await.unwrap();

        let script = Script::builder()
            .name("test name".to_string())
            .description("test description".to_string())
            .shebang("test shebang".to_string())
            .script("test script".to_string())
            .build();

        db.save(&script).await.unwrap();

        let loaded = db.load(&script.id.to_string()).await.unwrap().unwrap();

        assert_eq!(loaded, script);
    }

    #[tokio::test]
    async fn test_save_bulk() {
        let db = Database::new("sqlite::memory:", 1.0).await.unwrap();

        let scripts = vec![
            Script::builder()
                .name("test name".to_string())
                .description("test description".to_string())
                .shebang("test shebang".to_string())
                .script("test script".to_string())
                .build(),
            Script::builder()
                .name("test name 2".to_string())
                .description("test description 2".to_string())
                .shebang("test shebang 2".to_string())
                .script("test script 2".to_string())
                .build(),
        ];

        db.save_bulk(&scripts).await.unwrap();

        let loaded = db.list().await.unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "test name");
        assert_eq!(loaded[1].name, "test name 2");
    }

    #[tokio::test]
    async fn test_delete() {
        let db = Database::new("sqlite::memory:", 1.0).await.unwrap();

        let script = Script::builder()
            .name("test name".to_string())
            .description("test description".to_string())
            .shebang("test shebang".to_string())
            .script("test script".to_string())
            .build();

        db.save(&script).await.unwrap();

        assert_eq!(db.list().await.unwrap().len(), 1);
        db.delete(&script.id.to_string()).await.unwrap();

        let loaded = db.list().await.unwrap();
        assert_eq!(loaded.len(), 0);
    }
}
