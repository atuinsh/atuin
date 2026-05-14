//! User-authored context files (`TERMINAL.md`).
//!
//! Context files are markdown documents that can embed shell commands for
//! dynamic content. Before each API request, context files are discovered
//! by walking the filesystem, commands are executed, and the interpolated
//! content is sent to the server as `config.user_contexts`.

pub(crate) mod interpolate;
mod walker;

use std::path::Path;

pub(crate) use walker::global_context_path;

/// A fully resolved user context, ready to include in an API request.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct UserContext {
    /// The path to the context file on disk.
    pub path: String,
    /// The interpolated content.
    pub data: String,
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
