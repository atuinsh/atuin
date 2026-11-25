//! What counts as content in a tree, for the renderer and for the backends that build trees.

use crate::backend::{Node, Role};

/// Whether `node` and everything under it are containers, runs of text, or links.
pub fn is_inline_shaped(node: &Node) -> bool {
    matches!(node.role, Role::Container | Role::InlineText | Role::Link)
        && node.children.iter().all(is_inline_shaped)
}

/// Whether `text` shows anything besides blanks and icons.
pub fn has_words(text: &str) -> bool {
    text.chars().any(|c| !c.is_whitespace() && !is_icon(c))
}

/// Whether `c` is a character of the Private Use Areas, which icon fonts such as VS Code's
/// codicons draw as pictures. It never carries words.
pub fn is_icon(c: char) -> bool {
    matches!(c, '\u{e000}'..='\u{f8ff}' | '\u{f0000}'..='\u{10ffff}')
}

/// Whether the name of an element with this role is content when the element has no text.
pub const fn uses_name(role: Role) -> bool {
    matches!(
        role,
        Role::Heading | Role::ListItem | Role::Cell | Role::ColumnHeader | Role::Label | Role::Link
    )
}
