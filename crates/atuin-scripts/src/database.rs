use std::path::Path;
use std::time::Duration;

use atuin_common::sqlite::{Sqlite, TableView};
use atuin_common::table;
use sqlx::sqlite::SqliteRow;
use sqlx::{Result, Row};
use tracing::{debug, instrument};

use crate::store::script::{Script, ScriptTag};

table!(Script {
    name: "scripts",
    key: "id",
    conflict: ignore,
    columns: {
        id          => |s| s.id.to_string(),
        name        => |s| s.name.as_str(),
        description => |s| s.description.as_str(),
        shebang     => |s| s.shebang.as_str(),
        script      => |s| s.script.as_str(),
    },
});

table!(ScriptTag {
    name: "script_tags",
    key: ["script_id", "tag"],
    conflict: ignore,
    columns: {
        script_id => |t| t.script_id.to_string(),
        tag       => |t| t.tag.as_str(),
    },
});

#[derive(Debug, Clone)]
pub struct Database {
    sqlite: Sqlite,
    scripts: TableView<Script>,
    tags: TableView<ScriptTag>,
}

impl Database {
    pub async fn new(path: impl AsRef<Path>, timeout: Duration) -> eyre::Result<Self> {
        let path = path.as_ref();
        debug!("opening script sqlite database at {:?}", path);

        let sqlite = Sqlite::builder()
            .file(path)
            .timeout(timeout)
            .with_migrations(sqlx::migrate!("./migrations"))
            .open()
            .await?;

        let scripts = TableView::new(sqlite.clone());
        let tags = TableView::new(sqlite.clone());
        Ok(Self {
            sqlite,
            scripts,
            tags,
        })
    }

    pub async fn in_memory(timeout: Duration) -> eyre::Result<Self> {
        let sqlite = Sqlite::builder()
            .memory()
            .timeout(timeout)
            .with_migrations(sqlx::migrate!("./migrations"))
            .open()
            .await?;

        let scripts = TableView::new(sqlite.clone());
        let tags = TableView::new(sqlite.clone());
        Ok(Self {
            sqlite,
            scripts,
            tags,
        })
    }

    pub async fn save(&self, s: &Script) -> Result<()> {
        debug!("saving script to sqlite");
        self.save_bulk(std::slice::from_ref(s)).await
    }

    #[instrument(level = "debug", skip(self, s), fields(count = s.len()))]
    pub async fn save_bulk(&self, s: &[Script]) -> Result<()> {
        if s.is_empty() {
            return Ok(());
        }

        let mut tx = self.sqlite.pool().begin().await?;

        self.scripts.insert_bulk_tx(&mut tx, s).await?;

        let tag_rows: Vec<ScriptTag> = s
            .iter()
            .flat_map(|script| {
                script.tags.iter().map(|tag| ScriptTag {
                    script_id: script.id,
                    tag: tag.clone(),
                })
            })
            .collect();
        self.tags.insert_bulk_tx(&mut tx, &tag_rows).await?;

        tx.commit().await?;

        Ok(())
    }

    fn query_script_tags(row: &SqliteRow) -> String {
        row.get("tag")
    }

    #[allow(dead_code)]
    async fn load(&self, id: &str) -> Result<Option<Script>> {
        debug!("loading script item {}", id);

        let res = sqlx::query_as::<_, Script>("select * from scripts where id = ?1")
            .bind(id)
            .fetch_optional(self.sqlite.pool())
            .await?;

        // intentionally not joining, don't want to duplicate the script data in memory a whole bunch.
        if let Some(mut script) = res {
            let tags = sqlx::query("select tag from script_tags where script_id = ?1")
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

        let mut res = sqlx::query_as::<_, Script>("select * from scripts")
            .fetch_all(self.sqlite.pool())
            .await?;

        // Fetch all the tags for each script
        for script in &mut res {
            let tags = sqlx::query("select tag from script_tags where script_id = ?1")
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

        self.tags.delete_all().await?;
        self.scripts.delete_all().await?;

        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        debug!("deleting script {}", id);

        self.scripts.delete(id).await?;

        // delete all the tags for the script (delete-by-FK, not by PK, so this
        // stays a bespoke query rather than `TableView::delete`)
        sqlx::query("delete from script_tags where script_id = ?1")
            .bind(id)
            .execute(self.tags.sqlite().pool())
            .await?;

        Ok(())
    }

    pub async fn update(&self, s: &Script) -> Result<()> {
        debug!("updating script {:?}", s);

        let mut tx = self.sqlite.pool().begin().await?;

        // Update the script's base fields
        sqlx::query(
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
        sqlx::query("delete from script_tags where script_id = ?1")
            .bind(s.id.to_string())
            .execute(&mut *tx)
            .await?;

        // Insert new tags
        for tag in &s.tags {
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
        let res = sqlx::query_as::<_, Script>("select * from scripts where name = ?1")
            .bind(name)
            .fetch_optional(self.sqlite.pool())
            .await?;

        let script = if let Some(mut script) = res {
            let tags = sqlx::query("select tag from script_tags where script_id = ?1")
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
    async fn test_save_bulk_with_tags(#[future] db: Database) {
        let db = db.await;

        let scripts = vec![
            Script::builder()
                .name("tagged one".to_string())
                .description("test description".to_string())
                .shebang("test shebang".to_string())
                .script("test script".to_string())
                .tags(vec!["a".to_string(), "b".to_string()])
                .build(),
            Script::builder()
                .name("tagged two".to_string())
                .description("test description 2".to_string())
                .shebang("test shebang 2".to_string())
                .script("test script 2".to_string())
                .tags(vec!["c".to_string()])
                .build(),
        ];

        db.save_bulk(&scripts).await.unwrap();

        let loaded = db.list().await.unwrap();
        assert_eq!(loaded.len(), 2);

        let mut first_tags = loaded[0].tags.clone();
        first_tags.sort();
        assert_eq!(first_tags, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(loaded[1].tags, vec!["c".to_string()]);
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
