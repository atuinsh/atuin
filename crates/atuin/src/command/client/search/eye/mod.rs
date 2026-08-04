//! Declarative search TUI on eye-declare — the in-progress replacement for
//! `interactive.rs`, gated behind `ATUIN_EYE_SEARCH=1`.
//!
//! Inline mode only for now: fullscreen, the pty-proxy popup overlay, and
//! captured-stdout invocations fall back to the ratatui path at the
//! `search.rs` seam via [`inline_height`] returning `None`.

mod app;
mod view;

use std::io::{IsTerminal, stdout};

use atuin_client::{
    database::{Database, current_context},
    history::store::HistoryStore,
    settings::Settings,
    theme::Theme,
};
use atuin_common::shell::Shell;
use eyre::Result;

use super::engines;

/// The inline height the eye path would run at, or `None` when this
/// invocation must use the ratatui path.
pub fn inline_height(settings: &Settings) -> Option<u16> {
    if !std::env::var("ATUIN_EYE_SEARCH").is_ok_and(|v| !v.is_empty() && v != "0") {
        return None;
    }
    // The pty-proxy popup overlay stays on the ratatui path.
    if std::env::var("ATUIN_PTY_PROXY_SOCKET").is_ok() || std::env::var("ATUIN_HEX_SOCKET").is_ok()
    {
        return None;
    }
    // Captured stdout (VAR=$(atuin search -i)) needs the fd guard from P4.
    if !stdout().is_terminal() {
        return None;
    }
    let height = if settings.shell_up_key_binding {
        settings
            .inline_height_shell_up_key_binding
            .unwrap_or(settings.inline_height)
    } else {
        settings.inline_height
    };
    if height == 0 {
        return None;
    }
    // A terminal shorter than the requested height forces fullscreen (P4).
    match crossterm::terminal::size() {
        Ok((_, rows)) if height < rows => Some(height),
        _ => None,
    }
}

// Not Send because the eye_declare driver holds the stdout lock across
// awaits; the CLI drives this from a single-threaded context, same as the
// ratatui path.
#[allow(clippy::future_not_send)]
pub async fn history(
    query: &[String],
    settings: &Settings,
    db: impl Database,
    _history_store: &HistoryStore,
    _theme: &Theme,
    inline_height: u16,
) -> Result<String> {
    let original_query = query.join(" ");

    let is_command_chaining = settings.command_chaining && {
        let trimmed = original_query.trim_end();
        trimmed.ends_with("&&") || trimmed.ends_with('|')
    };
    let search_input = if is_command_chaining {
        String::new()
    } else {
        original_query.clone()
    };

    let context = current_context().await?;

    let search_mode = if settings.shell_up_key_binding {
        settings
            .search_mode_shell_up_key_binding()
            .unwrap_or_else(|| settings.search_mode())
    } else {
        settings.search_mode()
    };
    let filter_mode = settings
        .filter_mode_shell_up_key_binding
        .filter(|_| settings.shell_up_key_binding)
        .unwrap_or_else(|| settings.default_filter_mode(context.git_root.is_some()));

    let search_app = app::SearchApp::new(
        search_input,
        settings,
        Box::new(db),
        engines::engine(search_mode, settings),
        context,
        filter_mode,
        inline_height,
    );

    let options =
        eye_declare::RunOptions::default().keyboard(eye_declare::KeyboardProtocol::Enhanced);
    let output = eye_declare::driver_tokio::run_with(search_app, options).await?;

    let accept_shell = matches!(
        Shell::from_env(),
        Shell::Zsh | Shell::Fish | Shell::Bash | Shell::Xonsh | Shell::Nu | Shell::Powershell
    );

    Ok(match output {
        app::Output::ReturnOriginal => String::new(),
        app::Output::ReturnQuery(input) => input,
        app::Output::Selection { command, execute } => {
            if is_command_chaining {
                format!("{} {}", original_query.trim_end(), command)
            } else if execute && accept_shell {
                format!("__atuin_accept__:{command}")
            } else {
                command
            }
        }
    })
}
