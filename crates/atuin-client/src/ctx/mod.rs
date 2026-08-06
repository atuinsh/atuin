//! Process-wide application context.
//!
//! A single [`AppCtx`] static holds the crate's effectively-global state (right
//! now just workspace resolution) so it lives in one discoverable place instead
//! of being sprinkled across modules as ad-hoc statics. Reach it via
//! [`app()`]: `ctx::app().workspace().git_root(cwd).await`.

mod app;
mod workspace;

pub use app::AppCtx;
pub use workspace::WorkspaceCtx;

use std::sync::LazyLock;

static APP: LazyLock<AppCtx> = LazyLock::new(AppCtx::new);

/// The process-wide [`AppCtx`].
#[must_use]
pub fn app() -> &'static AppCtx {
    &APP
}
