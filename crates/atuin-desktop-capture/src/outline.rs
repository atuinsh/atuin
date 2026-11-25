//! The raw structure of a window, for diagnosing what a read finds and skips.

use std::fmt;

use crate::backend::{Completeness, Role};
use crate::layout::Frame;

/// Why a read leaves an element and everything below it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// The platform reports the element as not showing.
    Hidden,
    /// The element lies outside the visible area of its window or document.
    OutOfView,
    /// The element is an application control or a field that must not be read.
    Unreadable,
    /// The element did not tell its role, so it could be a field that must not be read.
    UnknownRole,
}

/// One element of a window, without any of its text.
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineEntry {
    /// How many ancestors the element has inside the window.
    pub depth: usize,
    /// The role in the platform's own terms, such as `push button` or `AXButton`.
    pub native_role: String,
    pub role: Role,
    /// How many characters of text the element holds, on platforms that tell without
    /// handing over the text.
    pub characters: Option<usize>,
    pub frame: Option<Frame>,
    pub children: usize,
    /// Why a read skips the element. The entries below a skipped element are skipped too,
    /// regardless of what they say themselves.
    pub skipped: Option<Skip>,
}

/// The unpruned element tree of a window in depth-first order.
///
/// The `Display` form prints one indented line per element.
#[derive(Debug, Clone, PartialEq)]
pub struct Outline {
    pub entries: Vec<OutlineEntry>,
    /// Whether the outline holds the whole tree, or stopped at the requested size.
    pub completeness: Completeness,
}

impl fmt::Display for OutlineEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:depth$}{} {:?}", "", self.native_role, self.role, depth = self.depth)?;
        if let Some(characters) = self.characters {
            write!(f, " text={characters}")?;
        }
        if let Some(frame) = self.frame {
            write!(f, " @{:.0},{:.0} {:.0}x{:.0}", frame.x, frame.y, frame.width, frame.height)?;
        }
        write!(f, " children={}", self.children)?;
        match self.skipped {
            None => Ok(()),
            Some(Skip::Hidden) => f.write_str("  [skipped: not showing]"),
            Some(Skip::OutOfView) => f.write_str("  [skipped: out of view]"),
            Some(Skip::Unreadable) => f.write_str("  [skipped: controls]"),
            Some(Skip::UnknownRole) => f.write_str("  [skipped: unknown role]"),
        }
    }
}

impl fmt::Display for Outline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for entry in &self.entries {
            writeln!(f, "{entry}")?;
        }
        if self.completeness == Completeness::Truncated {
            writeln!(f, "(stopped after {} elements)", self.entries.len())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn an_outline_prints_one_indented_line_per_element() {
        let entry = OutlineEntry {
            depth: 0,
            native_role: "frame".to_owned(),
            role: Role::Window,
            characters: None,
            frame: Some(Frame {
                x: 10.0,
                y: 20.4,
                width: 300.0,
                height: 200.0,
            }),
            children: 2,
            skipped: None,
        };
        let outline = Outline {
            entries: vec![entry, OutlineEntry {
                depth: 1,
                native_role: "tool bar".to_owned(),
                role: Role::Controls,
                characters: Some(0),
                frame: None,
                children: 0,
                skipped: Some(Skip::Unreadable),
            }],
            completeness: Completeness::Truncated,
        };
        assert_eq!(
            outline.to_string(),
            "frame Window @10,20 300x200 children=2\n tool bar Controls text=0 children=0  \
             [skipped: controls]\n(stopped after 2 elements)\n"
        );
    }
}
