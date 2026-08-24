use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use atuin_common::sqlite::{Sqlite, TableView};
use atuin_common::table;
use futures::TryStreamExt;
use sqlx::Result;
use tracing::instrument;
use uuid::Uuid;

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
        let sqlite = Sqlite::builder()
            .file(path.as_ref())
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
        self.save_bulk(std::slice::from_ref(s)).await
    }

    #[instrument(level = "debug", skip_all, fields(count = tracing::field::Empty))]
    pub async fn save_bulk<'a>(&self, s: impl IntoIterator<Item = &'a Script>) -> Result<()> {
        let scripts: Vec<&Script> = s.into_iter().collect();
        tracing::Span::current().record("count", scripts.len());
        if scripts.is_empty() {
            return Ok(());
        }

        let mut tx = self.sqlite.pool().begin().await?;

        self.scripts.on(&mut tx).insert_bulk(scripts.iter().copied()).await?;

        let tag_rows: Vec<ScriptTag> = scripts
            .iter()
            .flat_map(|script| {
                script.tags.iter().map(|tag| ScriptTag {
                    script_id: script.id,
                    tag: tag.clone(),
                })
            })
            .collect();
        self.tags.on(&mut tx).insert_bulk(&tag_rows).await?;

        tx.commit().await?;

        Ok(())
    }

    #[cfg(test)]
    async fn load(&self, id: &str) -> Result<Option<Script>> {
        let res = self.scripts.get(id).await?;

        // intentionally not joining, don't want to duplicate the script data in memory a whole bunch.
        if let Some(mut script) = res {
            script.tags = self.tags.filter(id).map_ok(|t| t.tag).try_collect().await?;
            Ok(Some(script))
        } else {
            Ok(None)
        }
    }

    pub async fn list(&self) -> Result<Vec<Script>> {
        let scripts: Vec<Script> = self.scripts.all().try_collect().await?;

        let mut tags = self
            .tags
            .all_ordered()
            .try_fold(HashMap::<Uuid, Vec<String>>::new(), |mut acc, row: ScriptTag| async move {
                acc.entry(row.script_id).or_default().push(row.tag);
                Ok(acc)
            })
            .await?;

        Ok(scripts
            .into_iter()
            .map(|mut script| {
                script.tags = tags.remove(&script.id).unwrap_or_default();
                script
            })
            .collect())
    }

    pub async fn clear(&self) -> Result<()> {
        self.tags.delete_all().await?;
        self.scripts.delete_all().await?;
        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        self.scripts.delete(id).await?;
        self.tags.delete(id).await?;
        Ok(())
    }

    pub async fn update(&self, s: &Script) -> Result<()> {
        let mut tx = self.sqlite.pool().begin().await?;

        // Update the script's base fields.
        self.scripts.on(&mut tx).update_one(s).await?;

        // Delete all existing tags for this script
        self.tags.on(&mut tx).delete(s.id.to_string()).await?;

        // Insert new tags
        let tag_rows: Vec<ScriptTag> = s
            .tags
            .iter()
            .map(|tag| ScriptTag {
                script_id: s.id,
                tag: tag.clone(),
            })
            .collect();
        self.tags.on(&mut tx).insert_bulk(&tag_rows).await?;

        tx.commit().await?;

        Ok(())
    }

    pub async fn get_by_name(&self, name: &str) -> Result<Option<Script>> {
        let res = sqlx::query_as::<_, Script>("select * from scripts where name = ?1")
            .bind(name)
            .fetch_optional(self.sqlite.pool())
            .await?;

        let script = if let Some(mut script) = res {
            script.tags =
                self.tags.filter(script.id.to_string()).map_ok(|t| t.tag).try_collect().await?;
            Some(script)
        } else {
            None
        };

        Ok(script)
    }
}

#[cfg(test)]
mod test {
    use rstest::{fixture, rstest};

    use super::*;

    #[fixture]
    async fn db() -> Database {
        Database::in_memory(Duration::from_secs(1)).await.unwrap()
    }

    /// Distinct per-field values, so a column/`FromRow` mix-up fails the test.
    fn script(name: &str, tags: &[&str]) -> Script {
        Script::builder()
            .name(name.to_string())
            .description(format!("{name} desc"))
            .shebang(format!("#!{name}"))
            .script(format!("echo {name}"))
            .tags(tags.iter().map(|t| t.to_string()).collect())
            .build()
    }

    fn sorted(tags: &[String]) -> Vec<String> {
        let mut v = tags.to_vec();
        v.sort();
        v
    }

    // Tags are the one thing this layer adds over `TableView` (the `script_tags`
    // side table); vary the set to cover none / one / many-unsorted in one place.
    #[rstest]
    #[case(&[])]
    #[case(&["only"])]
    #[case(&["b", "a", "c"])]
    #[tokio::test]
    async fn load_roundtrips_fields_and_tags(#[future(awt)] db: Database, #[case] tags: &[&str]) {
        let s = script("s", tags);
        db.save(&s).await.unwrap();

        let loaded = db.load(&s.id.to_string()).await.unwrap().unwrap();
        assert_eq!(loaded.name, "s");
        assert_eq!(loaded.description, "s desc");
        let want: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
        assert_eq!(sorted(&loaded.tags), sorted(&want));
    }

    #[rstest]
    #[tokio::test]
    async fn save_bulk_persists_every_row_with_its_tags(#[future(awt)] db: Database) {
        db.save_bulk(&[script("a", &["x", "y"]), script("b", &[])]).await.unwrap();

        let mut loaded = db.list().await.unwrap();
        loaded.sort_by(|l, r| l.name.cmp(&r.name)); // `list` order is unspecified
        assert_eq!(loaded.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(sorted(&loaded[0].tags), ["x", "y"].map(String::from));
        assert!(loaded[1].tags.is_empty());
    }

    #[rstest]
    #[tokio::test]
    async fn delete_removes_the_script_and_its_tags(#[future(awt)] db: Database) {
        let s = script("s", &["a", "b"]);
        db.save(&s).await.unwrap();

        db.delete(&s.id.to_string()).await.unwrap();

        assert!(db.load(&s.id.to_string()).await.unwrap().is_none());
        // the `script_tags` rows must go too — this is why `delete` touches both tables
        assert!(db.tags.all().try_collect::<Vec<_>>().await.unwrap().is_empty());
    }

    #[rstest]
    #[tokio::test]
    async fn update_replaces_base_fields_and_tags(#[future(awt)] db: Database) {
        let mut s = script("before", &["a", "b"]);
        db.save(&s).await.unwrap();

        s.name = "after".into();
        s.tags = vec!["c".into()];
        db.update(&s).await.unwrap();

        let loaded = db.load(&s.id.to_string()).await.unwrap().unwrap();
        assert_eq!(loaded.name, "after");
        assert_eq!(loaded.tags, ["c".to_string()]); // old tags gone, new tag present
    }
}
