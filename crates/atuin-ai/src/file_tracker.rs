//! Tracks which files have been read in the current session, for freshness
//! checking before edits.
//!
//! The tracker records the content hash and mtime of each file at the time
//! it was last read. Before an edit, the tracker verifies the file hasn't
//! changed since the last read — catching both external modifications and
//! concurrent tool calls.
//!
//! Persisted as JSON in session metadata so it survives across CLI
//! invocations within the same logical session.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eyre::Result;
use serde::{Deserialize, Serialize};

/// Metadata key used for session_metadata persistence.
pub(crate) const METADATA_KEY: &str = "file_read_tracker";

/// State recorded for a single file read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FileReadState {
    /// Hash of the file contents at the time of the last read.
    pub content_hash: u64,
    /// File mtime (as milliseconds since epoch) at the time of the last read.
    /// Millisecond precision ensures sub-second modifications are detected.
    pub mtime_ms: i64,
}

/// Tracks file read state for freshness checking.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct FileReadTracker {
    reads: HashMap<PathBuf, FileReadState>,
}

/// Result of a freshness check.
pub(crate) enum FreshnessCheck {
    /// File is fresh — the content hasn't changed since the last read.
    Fresh,
    /// File has never been read in this session.
    NotRead,
    /// File has been modified since the last read.
    Stale,
}

impl FileReadTracker {
    /// Record that a file was read. Call this after a successful `read_file`
    /// execution. The `path` should be canonical (absolute, tilde-expanded).
    pub fn record_read(&mut self, path: PathBuf, content: &[u8], mtime: SystemTime) {
        let content_hash = hash_content(content);
        let mtime_ms = system_time_to_ms(mtime);

        self.reads.insert(
            path,
            FileReadState {
                content_hash,
                mtime_ms,
            },
        );
    }

    /// Check whether a file is fresh (unchanged since last read).
    ///
    /// Uses mtime as a fast path — only re-hashes if mtime differs.
    pub fn check_freshness(&self, path: &Path) -> Result<FreshnessCheck> {
        let state = match self.reads.get(path) {
            Some(s) => s,
            None => return Ok(FreshnessCheck::NotRead),
        };

        // Stat the file
        let metadata = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => return Ok(FreshnessCheck::Stale), // file deleted or inaccessible
        };

        let current_mtime_ms =
            system_time_to_ms(metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH));

        // Fast path: mtime unchanged → fresh
        if current_mtime_ms == state.mtime_ms {
            return Ok(FreshnessCheck::Fresh);
        }

        // Mtime changed — re-hash to confirm
        let content = std::fs::read(path)?;
        let current_hash = hash_content(&content);

        if current_hash == state.content_hash {
            Ok(FreshnessCheck::Fresh)
        } else {
            Ok(FreshnessCheck::Stale)
        }
    }

    /// Update the tracker entry after a successful edit (new content written).
    pub fn update_after_edit(&mut self, path: &Path, new_content: &[u8], new_mtime: SystemTime) {
        let content_hash = hash_content(new_content);
        let mtime_ms = system_time_to_ms(new_mtime);

        self.reads.insert(
            path.to_path_buf(),
            FileReadState {
                content_hash,
                mtime_ms,
            },
        );
    }

    /// Serialize to JSON for session metadata persistence.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    /// Deserialize from JSON session metadata.
    pub fn from_json(json: &str) -> Result<Self> {
        Ok(serde_json::from_str(json)?)
    }
}

fn system_time_to_ms(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn hash_content(content: &[u8]) -> u64 {
    xxhash_rust::xxh3::xxh3_64(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn record_and_check_fresh() {
        let mut tracker = FileReadTracker::default();
        let mut tmp = NamedTempFile::new().unwrap();
        write!(tmp, "hello world").unwrap();

        let path = tmp.path().to_path_buf();
        let content = std::fs::read(&path).unwrap();
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();

        tracker.record_read(path.clone(), &content, mtime);

        assert!(matches!(
            tracker.check_freshness(&path).unwrap(),
            FreshnessCheck::Fresh
        ));
    }

    #[test]
    fn check_not_read() {
        let tracker = FileReadTracker::default();
        let path = PathBuf::from("/nonexistent/file.txt");
        assert!(matches!(
            tracker.check_freshness(&path).unwrap(),
            FreshnessCheck::NotRead
        ));
    }

    #[test]
    fn check_stale_after_modification() {
        let mut tracker = FileReadTracker::default();
        let mut tmp = NamedTempFile::new().unwrap();
        write!(tmp, "original").unwrap();

        let path = tmp.path().to_path_buf();
        let content = std::fs::read(&path).unwrap();
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();

        tracker.record_read(path.clone(), &content, mtime);

        // Small delay to ensure the filesystem mtime advances
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Modify the file
        std::fs::write(&path, "modified").unwrap();

        assert!(matches!(
            tracker.check_freshness(&path).unwrap(),
            FreshnessCheck::Stale
        ));
    }

    #[test]
    fn update_after_edit_makes_fresh() {
        let mut tracker = FileReadTracker::default();
        let mut tmp = NamedTempFile::new().unwrap();
        write!(tmp, "original").unwrap();

        let path = tmp.path().to_path_buf();
        let content = std::fs::read(&path).unwrap();
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();

        tracker.record_read(path.clone(), &content, mtime);

        // Simulate an edit
        let new_content = b"edited content";
        std::fs::write(&path, new_content).unwrap();
        let new_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        tracker.update_after_edit(&path, new_content, new_mtime);

        assert!(matches!(
            tracker.check_freshness(&path).unwrap(),
            FreshnessCheck::Fresh
        ));
    }

    #[test]
    fn roundtrip_json() {
        let mut tracker = FileReadTracker::default();
        tracker.reads.insert(
            PathBuf::from("/some/file.toml"),
            FileReadState {
                content_hash: 12345,
                mtime_ms: 1700000000000,
            },
        );

        let json = tracker.to_json().unwrap();
        let restored = FileReadTracker::from_json(&json).unwrap();
        assert_eq!(restored.reads.len(), 1);
        assert_eq!(
            restored.reads[&PathBuf::from("/some/file.toml")].content_hash,
            12345
        );
    }
}
