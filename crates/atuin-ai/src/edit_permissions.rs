//! Session-scoped permission cache for file edits.
//!
//! When the user selects "Allow this file for this session", the grant is
//! recorded here with a timestamp. Subsequent edits to the same file skip
//! the permission prompt as long as the grant hasn't expired.
//!
//! Grants are time-limited (1 hour TTL) so they don't outlive the user's
//! attention in long-running sessions. Persisted as JSON in session
//! metadata so they survive across CLI invocations.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eyre::Result;
use serde::{Deserialize, Serialize};

/// Session metadata key for persistence.
pub(crate) const METADATA_KEY: &str = "edit_permissions";

/// How long a session-scoped edit permission remains valid.
const TTL_MS: i64 = 60 * 60 * 1000; // 1 hour

/// Cache of per-file edit permission grants within a session.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct EditPermissionCache {
    /// Maps canonical file paths to the grant timestamp (unix millis).
    grants: HashMap<PathBuf, i64>,
}

impl EditPermissionCache {
    /// Record a permission grant for a file.
    pub fn grant(&mut self, path: PathBuf) {
        self.grants.insert(path, now_ms());
    }

    /// Check whether there's a valid (non-expired) grant for a file.
    pub fn has_valid_grant(&self, path: &Path) -> bool {
        if let Some(&granted_at) = self.grants.get(path) {
            (now_ms() - granted_at) < TTL_MS
        } else {
            false
        }
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    pub fn from_json(json: &str) -> Result<Self> {
        Ok(serde_json::from_str(json)?)
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_and_check() {
        let mut cache = EditPermissionCache::default();
        let path = PathBuf::from("/Users/me/.config/foo.toml");

        assert!(!cache.has_valid_grant(&path));
        cache.grant(path.clone());
        assert!(cache.has_valid_grant(&path));
    }

    #[test]
    fn different_paths_are_independent() {
        let mut cache = EditPermissionCache::default();
        let a = PathBuf::from("/etc/hosts");
        let b = PathBuf::from("/etc/resolv.conf");

        cache.grant(a.clone());
        assert!(cache.has_valid_grant(&a));
        assert!(!cache.has_valid_grant(&b));
    }

    #[test]
    fn roundtrip_json() {
        let mut cache = EditPermissionCache::default();
        cache.grant(PathBuf::from("/some/file.toml"));

        let json = cache.to_json().unwrap();
        let restored = EditPermissionCache::from_json(&json).unwrap();
        assert!(restored.has_valid_grant(Path::new("/some/file.toml")));
    }

    #[test]
    fn expired_grant_is_invalid() {
        let mut cache = EditPermissionCache::default();
        let path = PathBuf::from("/expired/file.toml");

        // Insert a grant from 2 hours ago
        let two_hours_ago = now_ms() - (2 * 60 * 60 * 1000);
        cache.grants.insert(path.clone(), two_hours_ago);

        assert!(!cache.has_valid_grant(&path));
    }
}
