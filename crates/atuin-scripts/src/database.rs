use std::ffi::OsStr;
use std::time::Duration;

use atuin_common::db;
use atuin_common::db::sqlite::{Sqlite, SqliteBuilder};
use sqlx::sqlite::{SqlitePool, SqliteRow};
use sqlx::{Result, Row};
use tracing::{debug, instrument};

use crate::store::script::Script;

const SCRIPT_COLUMNS: &str = "id, name, description, shebang, script";

#[derive(Debug, Clone)]
pub struct Database {
    sqlite: Sqlite,
}

impl Database {
    pub async fn new(path: impl AsRef<OsStr>, timeout: Duration) -> eyre::Result<Self> {
        let path = path.as_ref();
        debug!("opening script sqlite database at {:?}", path);

        Self::from_builder(Sqlite::builder(path), timeout).await
    }

    pub async fn in_memory(timeout: Duration) -> eyre::Result<Self> {
        Self::from_builder(Sqlite::builder_in_memory(), timeout).await
    }

    async fn from_builder(builder: SqliteBuilder<'_>, timeout: Duration) -> eyre::Result<Self> {
        let sqlite = builder.timeout(timeout).regexp().open().await?;

        Self::setup_db(sqlite.pool()).await?;

        Ok(Self { sqlite })
    }

    pub async fn sqlite_version(&self) -> eyre::Result<semver::Version> {
        Ok(self.sqlite.info().await.version?)
    }

    async fn setup_db(pool: &SqlitePool) -> Result<()> {
        debug!("running sqlite database setup");

        db::migrate!(pool, "./migrations").await?;

        Ok(())
    }

    async fn save_raw(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, s: &Script) -> Result<()> {
        db::query(
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

        for tag in &s.tags {
            db::query(
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
        let mut tx = self.sqlite.pool().begin().await?;
        Self::save_raw(&mut tx, s).await?;
        tx.commit().await?;

        Ok(())
    }

    #[instrument(level = "debug", skip(self, s), fields(count = s.len()))]
    pub async fn save_bulk(&self, s: &[Script]) -> Result<()> {
        if s.is_empty() {
            return Ok(());
        }

        let mut tx = self.sqlite.pool().begin().await?;

        for i in s {
            Self::save_raw(&mut tx, i).await?;
        }

        tx.commit().await?;

        Ok(())
    }

    fn query_script_tags(row: &SqliteRow) -> String {
        row.get("tag")
    }

    #[allow(dead_code)]
    async fn load(&self, id: &str) -> Result<Option<Script>> {
        debug!("loading script item {}", id);

        let res = db::query_as::<_, Script>(sqlx::AssertSqlSafe(format!(
            "select {SCRIPT_COLUMNS} from scripts where id = ?1"
        )))
        .bind(id)
        .fetch_optional(self.sqlite.pool())
        .await?;

        // intentionally not joining, don't want to duplicate the script data in memory a whole bunch.
        if let Some(mut script) = res {
            let tags = db::query("select tag from script_tags where script_id = ?1")
                .bind(id)
                .map(|row| Self::query_script_tags(&row))
                .fetch_all(self.sqlite.pool())
                .await?;

            script.tags = tags;
            Ok(Some(script))
        } else {
            Ok(None)
        }
    }

    pub async fn list(&self) -> Result<Vec<Script>> {
        debug!("listing scripts");

        let mut res = db::query_as::<_, Script>(sqlx::AssertSqlSafe(format!(
            "select {SCRIPT_COLUMNS} from scripts"
        )))
        .fetch_all(self.sqlite.pool())
        .await?;

        // Fetch all the tags for each script
        for script in &mut res {
            let tags = db::query("select tag from script_tags where script_id = ?1")
                .bind(script.id.to_string())
                .map(|row| Self::query_script_tags(&row))
                .fetch_all(self.sqlite.pool())
                .await?;

            script.tags = tags;
        }

        Ok(res)
    }

    pub async fn clear(&self) -> Result<()> {
        debug!("clearing all scripts from sqlite");

        db::query("delete from script_tags").execute(self.sqlite.pool()).await?;
        db::query("delete from scripts").execute(self.sqlite.pool()).await?;

        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        debug!("deleting script {}", id);

        db::query("delete from scripts where id = ?1").bind(id).execute(self.sqlite.pool()).await?;

        // delete all the tags for the script
        db::query("delete from script_tags where script_id = ?1")
            .bind(id)
            .execute(self.sqlite.pool())
            .await?;

        Ok(())
    }

    pub async fn update(&self, s: &Script) -> Result<()> {
        debug!("updating script {:?}", s);

        let mut tx = self.sqlite.pool().begin().await?;

        // Update the script's base fields
        db::query(
            "update scripts set name = ?1, description = ?2, shebang = ?3, script = ?4 where id = \
             ?5",
        )
        .bind(s.name.as_str())
        .bind(s.description.as_str())
        .bind(s.shebang.as_str())
        .bind(s.script.as_str())
        .bind(s.id.to_string())
        .execute(&mut *tx)
        .await?;

        // Delete all existing tags for this script
        db::query("delete from script_tags where script_id = ?1")
            .bind(s.id.to_string())
            .execute(&mut *tx)
            .await?;

        // Insert new tags
        for tag in &s.tags {
            db::query(
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
        let res = db::query_as::<_, Script>(sqlx::AssertSqlSafe(format!(
            "select {SCRIPT_COLUMNS} from scripts where name = ?1"
        )))
        .bind(name)
        .fetch_optional(self.sqlite.pool())
        .await?;

        let script = if let Some(mut script) = res {
            let tags = db::query("select tag from script_tags where script_id = ?1")
                .bind(script.id.to_string())
                .map(|row| Self::query_script_tags(&row))
                .fetch_all(self.sqlite.pool())
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
    use rstest::*;

    use super::*;

    #[fixture]
    async fn db() -> Database {
        Database::in_memory(Duration::from_secs(1)).await.unwrap()
    }

    #[fixture]
    fn script(
        #[default("test")] name: impl Into<String>,
        #[default("test")] description: impl Into<String>,
        #[default("test")] shebang: impl Into<String>,
        #[default("test")] script_body: impl Into<String>,
    ) -> Script {
        Script::builder()
            .name(name.into())
            .description(description.into())
            .shebang(shebang.into())
            .script(script_body.into())
            .build()
    }

    #[rstest]
    #[tokio::test]
    async fn test_list(#[future] db: Database, script: Script) {
        let db = db.await;

        let scripts = db.list().await.unwrap();
        assert_eq!(scripts.len(), 0);

        db.save(&script).await.unwrap();

        let scripts = db.list().await.unwrap();
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].name, "test");
    }

    #[rstest]
    #[tokio::test]
    async fn test_save_load(
        #[future] db: Database,
        #[with("test name", "test description", "test shebang", "test script")] script: Script,
    ) {
        let db = db.await;

        db.save(&script).await.unwrap();

        let loaded = db.load(&script.id.to_string()).await.unwrap().unwrap();

        assert_eq!(loaded, script);
    }

    #[rstest]
    #[tokio::test]
    async fn test_save_bulk(#[future] db: Database) {
        let db = db.await;

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

    #[rstest]
    #[tokio::test]
    async fn test_delete(#[future] db: Database, script: Script) {
        let db = db.await;

        db.save(&script).await.unwrap();

        assert_eq!(db.list().await.unwrap().len(), 1);
        db.delete(&script.id.to_string()).await.unwrap();

        let loaded = db.list().await.unwrap();
        assert_eq!(loaded.len(), 0);
    }
}
