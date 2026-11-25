//! Which windows, and which groups inside a window, are dialogs.

use super::desktop::BundleIds;
use super::element::{AttributeNames, Element, Pid, str_of};
use super::quirks;

/// How many elements above the focus [`DialogProbe::enclosing_dialog`] looks for a dialog.
/// Firefox puts the focus of a page's alert six levels below the dialog.
const MAX_DIALOG_DEPTH: usize = 10;

/// The subroles of a group that is a dialog inside a window.
const DIALOG_GROUPS: [&str; 3] = ["AXDialog", "AXApplicationDialog", "AXApplicationAlertDialog"];

/// The attributes that one call of a [`DialogProbe`] fetches. The call returns their values in
/// this order.
#[derive(Clone, Copy)]
enum Attribute {
    Role,
    Subrole,
    Parent,
}

impl Attribute {
    const ALL: [Self; 3] = [Self::Role, Self::Subrole, Self::Parent];

    const fn name(self) -> &'static str {
        match self {
            Self::Role => "AXRole",
            Self::Subrole => "AXSubrole",
            Self::Parent => "AXParent",
        }
    }
}

/// Tells which windows and groups are dialogs.
pub struct DialogProbe {
    names: AttributeNames,
}

impl Default for DialogProbe {
    fn default() -> Self {
        Self {
            names: AttributeNames::new(Attribute::ALL.map(Attribute::name)),
        }
    }
}

impl DialogProbe {
    /// Whether `window`, a window of the application `pid`, is a dialog.
    ///
    /// A sheet is a dialog, and so is a window with the subrole of a dialog. Every window of a
    /// system agent that shows only prompts is a dialog, and so is Chrome's window that holds
    /// only a page's alert.
    pub fn is_dialog(&self, window: &Element, pid: Pid, bundles: &BundleIds) -> bool {
        let marked = window.attributes(&self.names).is_some_and(|values| {
            values.is(Attribute::Role as usize, "AXSheet")
                || values
                    .text(Attribute::Subrole as usize)
                    .is_some_and(|subrole| is_dialog_subrole(&str_of(&subrole)))
        });
        marked
            || bundles.matches(pid, quirks::shows_only_prompts)
            || quirks::holds_page_alert(window)
    }

    /// Returns the dialog group inside a window that holds `element`, or `element` itself when
    /// it is one.
    ///
    /// Safari and Firefox show a page's alert as a dialog group inside the window.
    pub fn enclosing_dialog(&self, element: Element) -> Option<Element> {
        let mut current = element;
        for _ in 0..MAX_DIALOG_DEPTH {
            let values = current.attributes(&self.names)?;
            if values.is(Attribute::Role as usize, "AXWindow") {
                return None;
            }
            let subrole = values.text(Attribute::Subrole as usize);
            if subrole.is_some_and(|subrole| is_dialog_group(&str_of(&subrole))) {
                return Some(current);
            }
            current = values.element(Attribute::Parent as usize)?;
        }
        None
    }
}

/// Whether an element with `subrole` is a group that is a dialog inside a window. For example,
/// a page's alert in Safari and Firefox is such a group.
fn is_dialog_group(subrole: &str) -> bool {
    DIALOG_GROUPS.contains(&subrole)
}

/// Whether a window with `subrole` is a dialog.
fn is_dialog_subrole(subrole: &str) -> bool {
    subrole == "AXSystemDialog" || is_dialog_group(subrole)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// The index of each attribute is its place among the names that the probe fetches.
    #[rstest]
    fn attributes_are_listed_in_their_order() {
        for (index, attribute) in Attribute::ALL.into_iter().enumerate() {
            assert_eq!(attribute as usize, index);
        }
    }

    #[rstest]
    #[case::system_dialog("AXSystemDialog", true)]
    #[case::dialog_group("AXDialog", true)]
    #[case::page_alert("AXApplicationAlertDialog", true)]
    #[case::standard_window("AXStandardWindow", false)]
    #[case::floating_window("AXFloatingWindow", false)]
    fn dialog_subroles(#[case] subrole: &str, #[case] dialog: bool) {
        assert_eq!(is_dialog_subrole(subrole), dialog);
    }
}
