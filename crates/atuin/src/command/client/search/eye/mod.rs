//! Declarative search TUI on eye-declare — the in-progress replacement for
//! `interactive.rs`, gated behind `ATUIN_EYE_SEARCH=1`.
//!
//! Inline and fullscreen (alt-screen) modes; the pty-proxy popup overlay
//! falls back to the ratatui path at the `search.rs` seam via [`mode`]
//! returning `None`. Captured stdout (`VAR=$(atuin search -i)`) is handled
//! by temporarily pointing fd 1 at the controlling terminal, which keeps
//! the engine's writes, crossterm's queries, and raw mode coherent — and
//! lets inline mode work under capture, which the ratatui path never did.

mod app;
mod state;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EyeMode {
    Inline(u16),
    Fullscreen,
}

/// How the eye path would run this invocation, or `None` for the ratatui
/// path.
pub fn mode(settings: &Settings) -> Option<EyeMode> {
    if !std::env::var("ATUIN_EYE_SEARCH").is_ok_and(|v| !v.is_empty() && v != "0") {
        return None;
    }
    // The pty-proxy popup overlay stays on the ratatui path.
    if std::env::var("ATUIN_PTY_PROXY_SOCKET").is_ok() || std::env::var("ATUIN_HEX_SOCKET").is_ok()
    {
        return None;
    }
    // Captured stdout needs the /dev/tty fd redirection, which is unix-only;
    // other platforms keep the ratatui path there for now.
    if !stdout().is_terminal() && !cfg!(unix) {
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
        return Some(EyeMode::Fullscreen);
    }
    // A terminal shorter than the requested height forces fullscreen,
    // mirroring the ratatui path.
    match crossterm::terminal::size() {
        Ok((_, rows)) if height < rows => Some(EyeMode::Inline(height)),
        _ => Some(EyeMode::Fullscreen),
    }
}

/// Points fd 1 at `/dev/tty` for the TUI's lifetime when stdout is captured
/// (command substitution), restoring the original stdout on drop — before
/// the selected command is printed to it. Redirecting the fd (rather than
/// handing the engine a different writer) keeps everything that assumes
/// "terminal == stdout" — engine writes, crossterm's CPR and size queries,
/// raw mode — coherent with zero special-casing downstream.
#[cfg(unix)]
struct StdoutTtyGuard {
    saved: std::os::fd::OwnedFd,
}

#[cfg(unix)]
impl StdoutTtyGuard {
    fn redirect() -> std::io::Result<Option<Self>> {
        use std::os::fd::AsFd;
        if stdout().is_terminal() {
            return Ok(None);
        }
        let tty = std::fs::File::options()
            .read(true)
            .write(true)
            .open("/dev/tty")?;
        let saved = rustix::io::dup(rustix::stdio::stdout())?;
        rustix::stdio::dup2_stdout(tty.as_fd())?;
        Ok(Some(Self { saved }))
    }
}

#[cfg(unix)]
impl Drop for StdoutTtyGuard {
    fn drop(&mut self) {
        use std::os::fd::AsFd;
        let _ = rustix::stdio::dup2_stdout(self.saved.as_fd());
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
    history_store: &HistoryStore,
    theme: &Theme,
    mode: EyeMode,
) -> Result<String> {
    #[cfg(unix)]
    let _tty_guard = StdoutTtyGuard::redirect()?;

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

    let (screen, initial_height) = match mode {
        EyeMode::Inline(height) => (eye_declare::ScreenMode::Inline, height),
        EyeMode::Fullscreen => (
            eye_declare::ScreenMode::AltScreen,
            crossterm::terminal::size().map_or(24, |(_, rows)| rows),
        ),
    };

    let search_app = app::SearchApp::new(
        search_input,
        settings,
        theme,
        Box::new(db),
        engines::engine(search_mode, settings),
        engines::engine(search_mode, settings),
        history_store.clone(),
        context,
        filter_mode,
        search_mode,
        initial_height,
        mode == EyeMode::Fullscreen,
    );

    // The ratatui path pushes these kitty flags blind (no support probe);
    // matching it keeps modified-key reporting — and therefore user
    // keybindings — identical. Windows never pushed them there, so keep
    // the probing default on Windows.
    #[cfg(not(windows))]
    let keyboard = eye_declare::KeyboardProtocol::Custom {
        flags: crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | crossterm::event::KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
            | crossterm::event::KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS,
        probe: false,
    };
    #[cfg(windows)]
    let keyboard = eye_declare::KeyboardProtocol::Enhanced;

    let options = eye_declare::RunOptions::default()
        .keyboard(keyboard)
        .screen(screen)
        .mouse_capture(!settings.no_mouse);
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
