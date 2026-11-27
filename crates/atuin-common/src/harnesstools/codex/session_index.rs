use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use futures::StreamExt;
use jiff::Timestamp;
use serde::Deserialize;
use tokio::sync::watch;

use crate::fs::tree_watcher::TreeWatcher;
use crate::io::{LineReader, ReadLines};
use crate::sync::BlockingPool;

const FILE_NAME: &str = "session_index.jsonl";

pub(super) type Names = HashMap<String, Name>;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub(super) struct Name {
    pub(super) thread_name: String,
    pub(super) updated_at: Timestamp,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    #[serde(flatten)]
    name: Name,
}

pub(super) fn path(root: &Path) -> Option<PathBuf> {
    (root.file_name()? == "sessions").then(|| root.with_file_name(FILE_NAME))
}

/// The last complete entry for each thread wins, in the order Codex appended them.
pub(super) fn read(path: &Path) -> io::Result<Names> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Names::new()),
        Err(err) => return Err(err),
    };
    let mut reader = LineReader::new(file);
    let mut names = Names::new();
    for line in reader.lines().map_err(io::Error::other)? {
        if let Ok(entry) = serde_json::from_slice::<Entry>(&line?.bytes)
            && !entry.name.thread_name.trim().is_empty()
        {
            names.insert(entry.id, entry.name);
        }
    }
    Ok(names)
}

pub(super) async fn follow(path: PathBuf, pool: BlockingPool, names: watch::Sender<Names>) {
    let home = path.parent().filter(|home| !home.as_os_str().is_empty());
    let files = TreeWatcher::builder()
        .recursive(false)
        .filter(|path| path.file_name().is_some_and(|name| name == FILE_NAME))
        .watch(home.unwrap_or_else(|| Path::new(".")));
    let mut files = match files {
        Ok(files) => files,
        Err(err) => {
            tracing::warn!(%err, "could not watch the Codex session index");
            return;
        }
    };
    while let Some(file) = files.next().await {
        let (path, mut changes) = file.into_parts();
        loop {
            let path = path.clone();
            match pool.run(move || read(&path)).await.map_err(io::Error::other).and_then(|r| r) {
                Ok(current) => {
                    names.send_if_modified(|previous| {
                        if *previous == current {
                            return false;
                        }
                        *previous = current;
                        true
                    });
                }
                Err(err) => tracing::warn!(%err, "could not read the Codex session index"),
            }
            if changes.changed().await.is_err() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn reads_complete_names_in_append_order() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(FILE_NAME);
        assert!(read(&path).unwrap().is_empty());
        std::fs::write(
            &path,
            concat!(
                r#"{"id":"a","thread_name":"First","updated_at":"2026-11-21T09:11:00Z"}"#,
                "\n",
                "invalid record\n",
                r#"{"id":"b","thread_name":"Other thread","updated_at":"2026-11-21T09:11:00Z"}"#,
                "\n",
                r#"{"id":"a","thread_name":"Latest","updated_at":"2026-11-21T09:10:00Z"}"#,
                "\n",
                r#"{"id":"a","thread_name":"Unfinished","updated_at":"2026-11-21T09:12:00Z"}"#,
            ),
        )
        .unwrap();
        let names = read(&path).unwrap();
        assert_eq!(names.len(), 2);
        assert_eq!(names["a"].thread_name, "Latest");
        assert_eq!(names["b"].thread_name, "Other thread");
    }
}
