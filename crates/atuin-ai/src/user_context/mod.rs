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
use std::sync::Arc;

use parking_lot::Mutex;

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
    inner: Arc<Mutex<CacheSlot>>,
}

#[derive(Debug, Default)]
struct CacheSlot {
    /// Bumped on every invalidation, so a gather that raced with `/reload`
    /// can detect its result is stale and decline to cache it.
    epoch: u64,
    contexts: Option<Vec<UserContext>>,
}

impl UserContextCache {
    /// Return the cached contexts, gathering them first if the cache is empty.
    pub async fn get_or_gather(
        &self,
        start: &Path,
        global_path: Option<&Path>,
        shell: &str,
    ) -> Vec<UserContext> {
        // The lock is not held across the gather; streams run one at a time
        // so duplicate gathers aren't a concern.
        let epoch = {
            let slot = self.inner.lock();
            if let Some(contexts) = slot.contexts.clone() {
                return contexts;
            }
            slot.epoch
        };

        let contexts = gather(start, global_path, shell).await;

        // If `/reload` arrived mid-gather, this result predates it: return
        // it for the in-flight request but leave the cache empty so the
        // next request re-gathers. Likewise, never overwrite a result a
        // concurrent gather stored first — ours may be the older read.
        let mut slot = self.inner.lock();
        if slot.epoch == epoch && slot.contexts.is_none() {
            slot.contexts = Some(contexts.clone());
        }
        contexts
    }

    /// Drop the cached contexts so the next request re-gathers them.
    pub fn invalidate(&self) {
        let mut slot = self.inner.lock();
        slot.epoch += 1;
        slot.contexts = None;
    }
}

/// Discover context files and interpolate embedded commands.
///
/// Walks from `start` up to the filesystem root looking for
/// `.atuin/TERMINAL.md`, then checks `global_path`. Returns contexts
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

    #[tokio::test]
    async fn invalidate_during_gather_is_not_lost() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("TERMINAL.md");
        // The embedded sleep holds the gather open while we invalidate.
        tokio::fs::write(&file, "!`sleep 0.5 && echo one`")
            .await
            .unwrap();

        let cache = UserContextCache::default();

        let in_flight = tokio::spawn({
            let cache = cache.clone();
            let start = dir.path().to_path_buf();
            async move { cache.get_or_gather(&start, None, "sh").await }
        });

        // Let the gather read its epoch and start interpolating, then
        // invalidate mid-flight.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        cache.invalidate();
        tokio::fs::write(&file, "!`echo two`").await.unwrap();

        // The in-flight request still gets its pre-reload result...
        let contexts = in_flight.await.unwrap();
        assert_eq!(find(&contexts, &file).unwrap().data.trim(), "one");

        // ...but it must not repopulate the cache: the next request
        // re-gathers and sees the new content.
        let contexts = cache.get_or_gather(dir.path(), None, "sh").await;
        assert_eq!(find(&contexts, &file).unwrap().data.trim(), "two");
    }

    #[tokio::test]
    async fn slow_gather_does_not_overwrite_concurrent_result() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("TERMINAL.md");
        tokio::fs::write(&file, "!`sleep 0.5 && echo one`")
            .await
            .unwrap();

        let cache = UserContextCache::default();

        // First gather reads the old file and is held open by the sleep.
        let slow = tokio::spawn({
            let cache = cache.clone();
            let start = dir.path().to_path_buf();
            async move { cache.get_or_gather(&start, None, "sh").await }
        });

        // While it runs, the file changes and a second request gathers
        // and caches the new content.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        tokio::fs::write(&file, "!`echo two`").await.unwrap();
        let contexts = cache.get_or_gather(dir.path(), None, "sh").await;
        assert_eq!(find(&contexts, &file).unwrap().data.trim(), "two");

        // The slow gather finishes last with its older read...
        let contexts = slow.await.unwrap();
        assert_eq!(find(&contexts, &file).unwrap().data.trim(), "one");

        // ...but must not replace the newer cached result.
        let contexts = cache.get_or_gather(dir.path(), None, "sh").await;
        assert_eq!(find(&contexts, &file).unwrap().data.trim(), "two");
    }
}
