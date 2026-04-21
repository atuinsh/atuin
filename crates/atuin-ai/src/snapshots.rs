//! Backup snapshots for files before AI edits.
//!
//! Before the first edit to a file within a session, a snapshot of the
//! original content is saved so the user can recover if needed. Snapshots
//! are stored alongside a manifest that maps sanitized filenames back to
//! their original paths.
//!
//! Filenames use percent-encoding (`/` → `%2F`) so the snapshot directory
//! is human-readable via `ls`.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use eyre::{Result, eyre};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Snapshot store for a single session.
///
/// Each session gets its own directory under the snapshots root:
/// `<data_dir>/ai/snapshots/<session_id>/`
///
/// Files are stored with percent-encoded filenames derived from their
/// canonical paths, alongside a `manifest.json` that maps filenames
/// back to original paths with timestamps.
#[derive(Debug)]
pub(crate) struct SnapshotStore {
    session_dir: PathBuf,
    manifest: SnapshotManifest,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SnapshotManifest {
    files: HashMap<String, SnapshotEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SnapshotEntry {
    original_path: String,
    snapshot_at: String,
    size_bytes: u64,
}

impl SnapshotStore {
    /// Open or create a snapshot store for the given session directory.
    ///
    /// If a manifest already exists (from a prior CLI invocation in the same
    /// session), it's loaded so we don't re-snapshot files that were already
    /// backed up.
    pub fn open(session_dir: PathBuf) -> Result<Self> {
        let manifest_path = session_dir.join("manifest.json");
        let manifest = if manifest_path.exists() {
            let data = fs_err::read_to_string(&manifest_path)?;
            serde_json::from_str(&data)?
        } else {
            SnapshotManifest::default()
        };

        Ok(Self {
            session_dir,
            manifest,
        })
    }

    /// Snapshot a file's contents if it hasn't been snapshotted yet this session.
    ///
    /// Returns `true` if a new snapshot was created, `false` if one already
    /// existed. The `canonical_path` should be absolute (already tilde-expanded
    /// and resolved).
    pub fn ensure_snapshot(&mut self, canonical_path: &Path, content: &[u8]) -> Result<bool> {
        let filename = sanitize_path(canonical_path);

        if self.manifest.files.contains_key(&filename) {
            return Ok(false);
        }

        fs_err::create_dir_all(&self.session_dir)?;

        let snapshot_path = self.session_dir.join(&filename);
        atomic_write_file(&snapshot_path, content)?;

        let now = OffsetDateTime::now_utc();
        let entry = SnapshotEntry {
            original_path: canonical_path.to_string_lossy().into_owned(),
            snapshot_at: format_iso8601(now),
            size_bytes: content.len() as u64,
        };

        self.manifest.files.insert(filename, entry);
        self.save_manifest()?;

        Ok(true)
    }

    /// Whether a file has already been snapshotted in this session.
    #[cfg(test)]
    pub fn has_snapshot(&self, canonical_path: &Path) -> bool {
        let filename = sanitize_path(canonical_path);
        self.manifest.files.contains_key(&filename)
    }

    fn save_manifest(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.manifest)?;
        atomic_write_file(&self.session_dir.join("manifest.json"), json.as_bytes())
    }
}

/// Percent-encode a path for use as a filename.
///
/// Encodes `%` as `%25`, `/` as `%2F`, and `\` as `%5C`, then strips
/// leading separators and drive prefixes (e.g. `C:\`). The result is
/// always a flat filename safe for use with `Path::join` on any platform.
///
/// Example (Unix): `/Users/me/.config/foo.toml` → `Users%2Fme%2F.config%2Ffoo.toml`
/// Example (Windows): `C:\Users\me\config.toml` → `Users%5Cme%5Cconfig.toml`
pub(crate) fn sanitize_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    // Strip drive letter prefix on Windows (e.g. "C:\")
    let s = s.strip_prefix('/').unwrap_or_else(|| {
        // Handle Windows drive prefix like "C:\" or "C:/"
        if s.len() >= 3
            && s.as_bytes()[0].is_ascii_alphabetic()
            && s.as_bytes()[1] == b':'
            && (s.as_bytes()[2] == b'\\' || s.as_bytes()[2] == b'/')
        {
            &s[3..]
        } else {
            &s
        }
    });
    s.replace('%', "%25")
        .replace('/', "%2F")
        .replace('\\', "%5C")
}

/// Write a file atomically using temp-file-then-rename.
///
/// Creates a temporary file in the same directory as `target`, writes
/// content, fsyncs, then renames into place. Preserves permissions from
/// the original file if it exists.
pub(crate) fn atomic_write_file(target: &Path, content: &[u8]) -> Result<()> {
    let dir = target
        .parent()
        .ok_or_else(|| eyre!("target path has no parent directory"))?;
    fs_err::create_dir_all(dir)?;

    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(content)?;
    tmp.as_file().sync_all()?;

    // Preserve permissions from original if it exists
    if let Ok(meta) = std::fs::metadata(target) {
        std::fs::set_permissions(tmp.path(), meta.permissions())?;
    }

    tmp.persist(target).map_err(|e| {
        eyre!(
            "failed to persist atomic write to {}: {}",
            target.display(),
            e
        )
    })?;
    Ok(())
}

fn format_iso8601(dt: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        dt.year(),
        dt.month() as u8,
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── sanitize_path ──────────────────────────────────────────

    #[test]
    fn sanitize_absolute_path() {
        let path = Path::new("/Users/me/.config/atuin/config.toml");
        assert_eq!(
            sanitize_path(path),
            "Users%2Fme%2F.config%2Fatuin%2Fconfig.toml"
        );
    }

    #[test]
    fn sanitize_preserves_existing_percent() {
        let path = Path::new("/data/100%done/file.txt");
        assert_eq!(sanitize_path(path), "data%2F100%25done%2Ffile.txt");
    }

    #[test]
    fn sanitize_relative_path() {
        let path = Path::new("relative/path.txt");
        assert_eq!(sanitize_path(path), "relative%2Fpath.txt");
    }

    #[test]
    fn sanitize_no_collision_between_similar_paths() {
        let a = sanitize_path(Path::new("/foo/bar-baz"));
        let b = sanitize_path(Path::new("/foo/bar/baz"));
        assert_ne!(a, b);
    }

    #[test]
    fn sanitize_backslash_encoded() {
        // Windows-style path: backslashes become %5C, drive prefix stripped
        let s = sanitize_path(Path::new("C:\\Users\\me\\config.toml"));
        assert!(!s.contains('\\'), "backslashes must be encoded: {s}");
        assert!(!s.starts_with("C:"), "drive prefix must be stripped: {s}");
        assert!(s.contains("Users"));
        assert!(s.contains("config.toml"));
    }

    #[test]
    fn sanitize_result_is_flat_filename() {
        // The result must not be interpreted as a path with separators
        // when passed to Path::join — no raw / or \ allowed.
        let unix = sanitize_path(Path::new("/home/user/file.txt"));
        assert!(!unix.contains('/'));
        // Construct as if on Windows
        let win = "C:\\Users\\me\\file.txt";
        let encoded = win
            .strip_prefix("C:\\")
            .unwrap()
            .replace('%', "%25")
            .replace('/', "%2F")
            .replace('\\', "%5C");
        assert!(!encoded.contains('\\'));
        assert!(!encoded.contains('/'));
    }

    // ── atomic_write_file ──────────────────────────────────────

    #[test]
    fn atomic_write_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("test.txt");

        atomic_write_file(&target, b"hello world").unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello world");
    }

    #[test]
    fn atomic_write_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("test.txt");

        std::fs::write(&target, "old content").unwrap();
        atomic_write_file(&target, b"new content").unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new content");
    }

    #[test]
    fn atomic_write_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("sub").join("dir").join("test.txt");

        atomic_write_file(&target, b"nested").unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "nested");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("test.txt");

        std::fs::write(&target, "original").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();

        atomic_write_file(&target, b"updated").unwrap();

        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    // ── SnapshotStore ──────────────────────────────────────────

    #[test]
    fn snapshot_creates_file_and_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session-abc");
        let mut store = SnapshotStore::open(session_dir.clone()).unwrap();

        let file_path = Path::new("/Users/me/.config/foo.toml");
        let created = store
            .ensure_snapshot(file_path, b"[key]\nval = 1\n")
            .unwrap();

        assert!(created);
        assert!(store.has_snapshot(file_path));

        // Snapshot file on disk
        let expected_file = session_dir.join("Users%2Fme%2F.config%2Ffoo.toml");
        assert!(expected_file.exists());
        assert_eq!(
            std::fs::read_to_string(&expected_file).unwrap(),
            "[key]\nval = 1\n"
        );

        // Manifest on disk
        let manifest_path = session_dir.join("manifest.json");
        assert!(manifest_path.exists());
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        let files = manifest["files"].as_object().unwrap();
        assert_eq!(files.len(), 1);
        let entry = &files["Users%2Fme%2F.config%2Ffoo.toml"];
        assert_eq!(
            entry["original_path"].as_str().unwrap(),
            "/Users/me/.config/foo.toml"
        );
        assert_eq!(entry["size_bytes"].as_u64().unwrap(), 14);
    }

    #[test]
    fn snapshot_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session-abc");
        let mut store = SnapshotStore::open(session_dir.clone()).unwrap();

        let path = Path::new("/etc/hosts");
        let first = store.ensure_snapshot(path, b"first content").unwrap();
        let second = store.ensure_snapshot(path, b"different content").unwrap();

        assert!(first);
        assert!(!second);

        // Original content preserved, not overwritten
        let snapshot_file = session_dir.join("etc%2Fhosts");
        assert_eq!(
            std::fs::read_to_string(snapshot_file).unwrap(),
            "first content"
        );
    }

    #[test]
    fn snapshot_store_loads_existing_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session-abc");

        // First store: create a snapshot
        {
            let mut store = SnapshotStore::open(session_dir.clone()).unwrap();
            store
                .ensure_snapshot(Path::new("/etc/hosts"), b"127.0.0.1")
                .unwrap();
        }

        // Second store (simulates new CLI invocation): should see existing snapshot
        {
            let mut store = SnapshotStore::open(session_dir).unwrap();
            assert!(store.has_snapshot(Path::new("/etc/hosts")));

            let created = store
                .ensure_snapshot(Path::new("/etc/hosts"), b"new content")
                .unwrap();
            assert!(!created);
        }
    }

    #[test]
    fn snapshot_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session-abc");
        let mut store = SnapshotStore::open(session_dir.clone()).unwrap();

        store
            .ensure_snapshot(Path::new("/etc/hosts"), b"hosts content")
            .unwrap();
        store
            .ensure_snapshot(Path::new("/Users/me/.bashrc"), b"bashrc content")
            .unwrap();

        assert!(store.has_snapshot(Path::new("/etc/hosts")));
        assert!(store.has_snapshot(Path::new("/Users/me/.bashrc")));
        assert!(!store.has_snapshot(Path::new("/nonexistent")));

        // Both snapshot files exist
        assert!(session_dir.join("etc%2Fhosts").exists());
        assert!(session_dir.join("Users%2Fme%2F.bashrc").exists());

        // Manifest has both entries
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(session_dir.join("manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["files"].as_object().unwrap().len(), 2);
    }

    #[test]
    fn format_iso8601_produces_valid_format() {
        let dt = OffsetDateTime::from_unix_timestamp(1700000000).unwrap();
        let formatted = format_iso8601(dt);
        assert_eq!(formatted.len(), 20);
        assert!(formatted.starts_with("2023-"));
        assert!(formatted.contains('T'));
        assert!(formatted.ends_with('Z'));
    }
}
