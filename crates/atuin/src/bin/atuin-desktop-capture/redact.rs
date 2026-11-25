//! The secret filter that the helper applies to everything that it captures.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use atuin_common::secrets::{REDACTED, Redactor};
use atuin_desktop_capture::Redact;

/// How long the redaction of one text may take. A text that takes longer is dropped whole,
/// because keeping it unchecked is worse than losing it.
const REDACTION_BUDGET: Duration = Duration::from_secs(2);

/// Takes credentials out with the patterns that Atuin uses for shell history and output.
#[derive(Debug, Clone, Default)]
pub struct AtuinRedactor(pub Arc<Redactor>);

impl Redact for AtuinRedactor {
    fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        self.0.redact_within(text, REDACTION_BUDGET).unwrap_or(Cow::Borrowed(REDACTED))
    }
}
