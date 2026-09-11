//! Interactive terminal prompts, built on `inquire`.
//!
//! Every CLI prompt (text entry, passwords, y/N confirmations) funnels through
//! here so behaviour and styling stay consistent, and so non-interactive input
//! (piped stdin, CI) is handled the same way everywhere: a prompt that needs an
//! answer errors when stdin is not a terminal — callers expose a flag for the
//! value instead. Confirmations fall back to their default rather than erroring.

use std::io::{self, IsTerminal};

use eyre::{Result, bail, eyre};
use inquire::{Confirm, InquireError, Password, PasswordDisplayMode, Text, validator::Validation};

/// Map an [`InquireError`] to an [`eyre::Report`], turning a user cancellation
/// (Esc / Ctrl-C) into a plain "cancelled" error rather than a debug dump.
fn into_report(err: InquireError) -> eyre::Report {
    match err {
        InquireError::OperationCanceled | InquireError::OperationInterrupted => eyre!("cancelled"),
        other => eyre::Report::new(other),
    }
}

/// Bail unless stdin is a terminal, so we never block waiting on a prompt that
/// can't be answered. The message names what we were about to ask for.
fn require_terminal(what: &str) -> Result<()> {
    if io::stdin().is_terminal() {
        Ok(())
    } else {
        bail!(
            "cannot prompt for {what} because stdin is not a terminal; pass it as a flag instead"
        );
    }
}

/// Prompt for a line of text.
pub fn text(message: &str) -> Result<String> {
    require_terminal(message)?;
    Text::new(message).prompt().map_err(into_report)
}

/// Return `value` if present, otherwise prompt for it.
pub fn text_or(value: Option<String>, message: &str) -> Result<String> {
    value.map_or_else(|| text(message), Ok)
}

/// Prompt for a password without echoing it. No confirmation step.
pub fn password(message: &str) -> Result<String> {
    require_terminal(message)?;
    Password::new(message)
        .with_display_mode(PasswordDisplayMode::Hidden)
        .without_confirmation()
        .prompt()
        .map_err(into_report)
}

/// Prompt for a new password, requiring a non-empty value entered twice.
pub fn new_password(message: &str) -> Result<String> {
    require_terminal(message)?;
    Password::new(message)
        .with_display_mode(PasswordDisplayMode::Hidden)
        .with_custom_confirmation_message("Confirm password:")
        .with_custom_confirmation_error_message("The passwords don't match. Please try again.")
        .with_validator(|input: &str| {
            if input.is_empty() {
                Ok(Validation::Invalid("Password cannot be empty.".into()))
            } else {
                Ok(Validation::Valid)
            }
        })
        .prompt()
        .map_err(into_report)
}

/// Ask a yes/no question with the given default. Returns the default when stdin
/// is not a terminal.
pub fn confirm(message: &str, default: bool) -> Result<bool> {
    if !io::stdin().is_terminal() {
        return Ok(default);
    }
    Confirm::new(message)
        .with_default(default)
        .prompt()
        .map_err(into_report)
}

/// Guard a destructive action. Proceeds without asking when `assume_yes` is set
/// (e.g. `--force`) or when stdin is not a terminal, preserving scripted use;
/// otherwise asks for confirmation, defaulting to no.
pub fn confirm_destructive(message: &str, assume_yes: bool) -> Result<bool> {
    if assume_yes || !io::stdin().is_terminal() {
        return Ok(true);
    }
    Confirm::new(message)
        .with_default(false)
        .prompt()
        .map_err(into_report)
}
