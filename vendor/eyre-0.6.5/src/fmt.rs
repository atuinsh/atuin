use crate::error::ErrorImpl;
use core::fmt;

impl ErrorImpl<()> {
    pub(crate) fn display(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.handler
            .as_ref()
            .map(|handler| handler.display(self.error(), f))
            .unwrap_or_else(|| core::fmt::Display::fmt(self.error(), f))
    }

    pub(crate) fn debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.handler
            .as_ref()
            .map(|handler| handler.debug(self.error(), f))
            .unwrap_or_else(|| core::fmt::Debug::fmt(self.error(), f))
    }
}
