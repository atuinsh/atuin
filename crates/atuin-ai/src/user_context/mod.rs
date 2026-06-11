//! User-authored context files (`TERMINAL.md`).
//!
//! Context files are markdown documents that can embed shell commands for
//! dynamic content. On the first API request of an invocation, context files
//! are discovered by walking the filesystem, commands are executed, and the
//! interpolated content is sent to the server as `config.user_contexts`.
//! The result is cached for the rest of the invocation; `/reload` clears
//! the cache so the next request re-gathers.

pub(crate) mod interpolate;
mod walker;

use std::path::Path;
use std::sync::{Arc, Mutex};

pub(crate) use walker::global_context_path;

/// A fully resolved user context, ready to include in an API request.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct UserContext {
    /// The path to the context file on disk.
    pub path: String,
    /// The interpolated content.
    pub data: String,
}

/// Process-lifetime cache of gathered user contexts.
///
/// Context files are walked and interpolated once per invocation; subsequent
/// requests reuse the cached result. `/reload` invalidates the cache so the
/// next request re-gathers.
#[derive(Debug, Clone, Default)]
pub(crate) struct UserContextCache {
    inner: Arc<Mutex<Option<Vec<UserContext>>>>,
}

impl UserContextCache {
    /// Return the cached contexts, gathering them first if the cache is empty.
    pub async fn get_or_gather(
        &self,
        start: &Path,
        global_path: Option<&Path>,
        shell: &str,
    ) -> Vec<UserContext> {
        if let Some(contexts) = self.lock().clone() {
            return contexts;
        }

        // Concurrent callers may both gather here; streams run one at a
        // time so this stays simpler than holding a lock across .await.
        let contexts = gather(start, global_path, shell).await;
        *self.lock() = Some(contexts.clone());
        contexts
    }

    /// Drop the cached contexts so the next request re-gathers them.
    pub fn invalidate(&self) {
        self.lock().take();
    }

    /// A poisoned lock means another thread panicked while holding it, but
    /// the cached value is only ever replaced wholesale — it can't be torn.
    /// Recover with the inner value rather than propagating the panic.
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Vec<UserContext>>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Discover context files and interpolate embedded commands.
///
/// Walks from `start` up to the filesystem root looking for
/// `.atuin/ai-context.md`, then checks `global_path`. Returns contexts
/// ordered from most general (global/root) to most specific (deepest).
pub(crate) async fn gather(
    start: &Path,
    global_path: Option<&Path>,
    shell: &str,
) -> Vec<UserContext> {
    let raw_files = match walker::walk(start, global_path).await {
        Ok(files) => files,
        Err(e) => {
            tracing::warn!("Failed to walk for context files: {e}");
            return Vec::new();
        }
    };

    if raw_files.is_empty() {
        return Vec::new();
    }

    // Interpolate all files in parallel.
    let mut handles = Vec::with_capacity(raw_files.len());
    for file in raw_files {
        let shell = shell.to_string();
        handles.push(tokio::spawn(async move {
            let data = interpolate::interpolate(&file.content, &shell).await;
            UserContext {
                path: file.path.to_string_lossy().to_string(),
                data,
            }
        }));
    }

    let mut contexts = Vec::with_capacity(handles.len());
    for handle in handles {
        match handle.await {
            Ok(ctx) => contexts.push(ctx),
            Err(e) => tracing::warn!("Context interpolation task failed: {e}"),
        }
    }

    contexts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(contexts: &'a [UserContext], path: &Path) -> Option<&'a UserContext> {
        let path = path.to_string_lossy();
        contexts.iter().find(|c| c.path == path)
    }

    #[tokio::test]
    async fn cache_serves_stale_until_invalidated() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("TERMINAL.md");
        tokio::fs::write(&file, "version one").await.unwrap();

        let cache = UserContextCache::default();

        let contexts = cache.get_or_gather(dir.path(), None, "sh").await;
        assert_eq!(find(&contexts, &file).unwrap().data, "version one");

        tokio::fs::write(&file, "version two").await.unwrap();

        let contexts = cache.get_or_gather(dir.path(), None, "sh").await;
        assert_eq!(find(&contexts, &file).unwrap().data, "version one");

        cache.invalidate();

        let contexts = cache.get_or_gather(dir.path(), None, "sh").await;
        assert_eq!(find(&contexts, &file).unwrap().data, "version two");
    }
}
