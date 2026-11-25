//! The roles of AT-SPI by number, and what each one means to a read.
//!
//! Toolkits name roles in their own words. GTK 4 says `menu item radio` and `window` where GTK 3
//! says `radio menu item` and `frame`. The number that `GetRole` returns comes from one enum,
//! `AtspiRole` in `atspi-constants.h`, so it means the same in every toolkit.

use std::borrow::Cow;
use std::fmt;

use crate::backend::Role;

/// What the elements around an element change about how the element is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Within {
    Window,
    Document,
    /// A button outside documents. The only content of the button that is read is the menu
    /// that the button opened, when that menu is showing.
    Button,
    /// A menu that is showing, of which only the options and the labels are read.
    Menu,
}

impl Within {
    /// The read role of an element with `role` here.
    pub const fn role(self, role: AtspiRole) -> Role {
        match self {
            Self::Window if role.is_button() => Role::Container,
            Self::Window | Self::Document => role.role(),
            Self::Button if role.is_menu() => Role::Container,
            Self::Button => Role::Controls,
            Self::Menu => role.role_in_menu(),
        }
    }

    /// Where the children of an element with `role` here lie.
    pub fn inner(self, role: AtspiRole) -> Self {
        match self {
            Self::Window if role.is_button() => Self::Button,
            Self::Window if role.role() == Role::Document => Self::Document,
            Self::Button if role.is_menu() => Self::Menu,
            within => within,
        }
    }
}

/// A role as AT-SPI numbers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtspiRole(pub u32);

impl AtspiRole {
    pub const ALERT: Self = Self(2);
    pub const CAPTION: Self = Self(81);
    pub const CHECK_BOX: Self = Self(7);
    pub const CHECK_MENU_ITEM: Self = Self(8);
    pub const COLUMN_HEADER: Self = Self(10);
    pub const COMBO_BOX: Self = Self(11);
    pub const DIALOG: Self = Self(16);
    pub const DOCUMENT_EMAIL: Self = Self(96);
    pub const DOCUMENT_FRAME: Self = Self(82);
    pub const DOCUMENT_PRESENTATION: Self = Self(93);
    pub const DOCUMENT_SPREADSHEET: Self = Self(92);
    pub const DOCUMENT_TEXT: Self = Self(94);
    pub const DOCUMENT_WEB: Self = Self(95);
    pub const EDITBAR: Self = Self(77);
    pub const ENTRY: Self = Self(79);
    pub const FILE_CHOOSER: Self = Self(19);
    pub const FRAME: Self = Self(23);
    pub const HEADING: Self = Self(83);
    pub const ICON: Self = Self(26);
    /// The role that GTK 4 reports for an element that it leaves out of the tree. See
    /// [`super::quirks::leaves_roleless_elements_out`].
    pub const INVALID: Self = Self(0);
    pub const IMAGE: Self = Self(27);
    pub const LABEL: Self = Self(29);
    pub const LINK: Self = Self(88);
    pub const LIST: Self = Self(31);
    pub const LIST_ITEM: Self = Self(32);
    pub const MENU: Self = Self(33);
    pub const MENU_BAR: Self = Self(34);
    pub const MENU_ITEM: Self = Self(35);
    pub const NOTIFICATION: Self = Self(101);
    pub const PAGE_TAB: Self = Self(37);
    pub const PARAGRAPH: Self = Self(73);
    pub const PASSWORD_TEXT: Self = Self(40);
    pub const POPUP_MENU: Self = Self(41);
    pub const PROGRESS_BAR: Self = Self(42);
    /// The button that AT-SPI also calls `push button`.
    pub const BUTTON: Self = Self(43);
    pub const PUSH_BUTTON_MENU: Self = Self(129);
    pub const RADIO_BUTTON: Self = Self(44);
    pub const RADIO_MENU_ITEM: Self = Self(45);
    pub const ROW_HEADER: Self = Self(47);
    pub const SCROLL_BAR: Self = Self(48);
    pub const SECTION: Self = Self(85);
    pub const SEPARATOR: Self = Self(50);
    pub const SLIDER: Self = Self(51);
    pub const SPIN_BUTTON: Self = Self(52);
    pub const STATIC: Self = Self(116);
    pub const STATUS_BAR: Self = Self(54);
    pub const SWITCH: Self = Self(130);
    pub const TABLE_CELL: Self = Self(56);
    pub const TABLE_COLUMN_HEADER: Self = Self(57);
    pub const TABLE_ROW_HEADER: Self = Self(58);
    pub const TABLE_ROW: Self = Self(90);
    pub const TEAROFF_MENU_ITEM: Self = Self(59);
    pub const TERMINAL: Self = Self(60);
    pub const TEXT: Self = Self(61);
    pub const TOGGLE_BUTTON: Self = Self(62);
    pub const TOOL_BAR: Self = Self(63);
    pub const TOOL_TIP: Self = Self(64);
    pub const TREE_ITEM: Self = Self(91);
    pub const WINDOW: Self = Self(69);

    /// What the role means to a read outside a menu.
    #[must_use]
    pub const fn role(self) -> Role {
        match self {
            Self::FRAME | Self::WINDOW | Self::DIALOG | Self::ALERT | Self::FILE_CHOOSER => {
                Role::Window
            }
            Self::DOCUMENT_WEB
            | Self::DOCUMENT_FRAME
            | Self::DOCUMENT_TEXT
            | Self::DOCUMENT_EMAIL
            | Self::DOCUMENT_SPREADSHEET
            | Self::DOCUMENT_PRESENTATION => Role::Document,
            Self::HEADING => Role::Heading,
            Self::PARAGRAPH => Role::Paragraph,
            Self::LIST_ITEM => Role::ListItem,
            Self::TABLE_CELL | Self::ROW_HEADER | Self::TABLE_ROW_HEADER => Role::Cell,
            Self::COLUMN_HEADER | Self::TABLE_COLUMN_HEADER => Role::ColumnHeader,
            Self::TABLE_ROW => Role::Row,
            Self::LABEL
            | Self::STATUS_BAR
            | Self::PAGE_TAB
            | Self::TREE_ITEM
            | Self::NOTIFICATION
            | Self::CAPTION
            | Self::CHECK_BOX
            | Self::RADIO_BUTTON
            | Self::SWITCH
            // An icon view names its items, such as the printers or the files that it shows.
            // An icon inside a toolbar or a button is never reached.
            | Self::ICON => Role::Label,
            Self::STATIC => Role::InlineText,
            Self::LINK => Role::Link,
            Self::TEXT | Self::ENTRY | Self::EDITBAR | Self::SPIN_BUTTON => Role::Editable,
            Self::PASSWORD_TEXT => Role::Secret,
            Self::TERMINAL => Role::Terminal,
            Self::MENU_BAR
            | Self::MENU
            | Self::MENU_ITEM
            | Self::CHECK_MENU_ITEM
            | Self::RADIO_MENU_ITEM
            | Self::TEAROFF_MENU_ITEM
            | Self::POPUP_MENU
            | Self::TOOL_BAR
            | Self::SCROLL_BAR
            | Self::SEPARATOR
            | Self::BUTTON
            | Self::TOGGLE_BUTTON
            | Self::COMBO_BOX
            | Self::SLIDER
            | Self::IMAGE
            | Self::TOOL_TIP
            | Self::PUSH_BUTTON_MENU
            | Self::PROGRESS_BAR => Role::Controls,
            _ => Role::Container,
        }
    }

    /// What the role means to a read inside a menu that is showing. The menu's options become
    /// options, and the menu itself holds them. Commands stay controls.
    #[must_use]
    pub const fn role_in_menu(self) -> Role {
        match self {
            Self::MENU | Self::POPUP_MENU => Role::Container,
            Self::CHECK_MENU_ITEM | Self::RADIO_MENU_ITEM => Role::Label,
            _ => self.role(),
        }
    }

    /// Whether an element with this role is an option such as a check box, whose state is
    /// read, when the element lies `within` the elements around it.
    #[must_use]
    pub const fn is_option(self, within: Within) -> bool {
        match self {
            Self::CHECK_BOX | Self::RADIO_BUTTON | Self::SWITCH => true,
            Self::CHECK_MENU_ITEM | Self::RADIO_MENU_ITEM => matches!(within, Within::Menu),
            _ => false,
        }
    }

    /// Whether the role is a menu, which a button can hold when the menu is open.
    #[must_use]
    pub const fn is_menu(self) -> bool {
        matches!(self, Self::MENU | Self::POPUP_MENU)
    }

    /// Whether the role is a button, which can hold the menu that the button opened.
    #[must_use]
    pub const fn is_button(self) -> bool {
        matches!(self, Self::BUTTON | Self::TOGGLE_BUTTON | Self::PUSH_BUTTON_MENU)
    }

    /// The role's name in AT-SPI's own words, such as `radio menu item`. `None` for a number
    /// that AT-SPI does not define.
    #[must_use]
    pub fn name(self) -> Option<&'static str> {
        NAMES.get(usize::try_from(self.0).ok()?).copied()
    }
}

impl AtspiRole {
    /// The role's name for [`Node::native_role`](crate::backend::Node::native_role). A role
    /// that AT-SPI does not define gets a name with its number.
    #[must_use]
    pub fn native_name(self) -> Cow<'static, str> {
        self.name().map_or_else(|| Cow::Owned(self.to_string()), Cow::Borrowed)
    }
}

impl fmt::Display for AtspiRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "role {}", self.0),
        }
    }
}

/// The names of the roles, in the order of `AtspiRole`. Each name is the role's identifier in
/// `atspi-constants.h`, in lowercase and with spaces.
const NAMES: [&str; 131] = [
    "invalid",
    "accelerator label",
    "alert",
    "animation",
    "arrow",
    "calendar",
    "canvas",
    "check box",
    "check menu item",
    "color chooser",
    "column header",
    "combo box",
    "date editor",
    "desktop icon",
    "desktop frame",
    "dial",
    "dialog",
    "directory pane",
    "drawing area",
    "file chooser",
    "filler",
    "focus traversable",
    "font chooser",
    "frame",
    "glass pane",
    "html container",
    "icon",
    "image",
    "internal frame",
    "label",
    "layered pane",
    "list",
    "list item",
    "menu",
    "menu bar",
    "menu item",
    "option pane",
    "page tab",
    "page tab list",
    "panel",
    "password text",
    "popup menu",
    "progress bar",
    "button",
    "radio button",
    "radio menu item",
    "root pane",
    "row header",
    "scroll bar",
    "scroll pane",
    "separator",
    "slider",
    "spin button",
    "split pane",
    "status bar",
    "table",
    "table cell",
    "table column header",
    "table row header",
    "tearoff menu item",
    "terminal",
    "text",
    "toggle button",
    "tool bar",
    "tool tip",
    "tree",
    "tree table",
    "unknown",
    "viewport",
    "window",
    "extended",
    "header",
    "footer",
    "paragraph",
    "ruler",
    "application",
    "autocomplete",
    "editbar",
    "embedded",
    "entry",
    "chart",
    "caption",
    "document frame",
    "heading",
    "page",
    "section",
    "redundant object",
    "form",
    "link",
    "input method window",
    "table row",
    "tree item",
    "document spreadsheet",
    "document presentation",
    "document text",
    "document web",
    "document email",
    "comment",
    "list box",
    "grouping",
    "image map",
    "notification",
    "info bar",
    "level bar",
    "title bar",
    "block quote",
    "audio",
    "video",
    "definition",
    "article",
    "landmark",
    "log",
    "marquee",
    "math",
    "rating",
    "timer",
    "static",
    "math fraction",
    "math root",
    "subscript",
    "superscript",
    "description list",
    "description term",
    "description value",
    "footnote",
    "content deletion",
    "content insertion",
    "mark",
    "suggestion",
    "push button menu",
    "switch",
];

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// The names line up with the numbers that applications answered.
    #[rstest]
    #[case::gtk3_radio_button(AtspiRole::RADIO_BUTTON, "radio button")]
    #[case::gtk3_toggle_button(AtspiRole::TOGGLE_BUTTON, "toggle button")]
    #[case::gtk4_menu_item_radio(AtspiRole::RADIO_MENU_ITEM, "radio menu item")]
    #[case::gtk4_window(AtspiRole::FRAME, "frame")]
    #[case::gtk4_generic(AtspiRole(39), "panel")]
    #[case::gtk4_group(AtspiRole(99), "grouping")]
    #[case::the_last(AtspiRole::SWITCH, "switch")]
    fn numbers_name_their_roles(#[case] role: AtspiRole, #[case] name: &str) {
        assert_eq!(role.name(), Some(name));
    }

    #[rstest]
    fn an_unknown_number_names_itself() {
        assert_eq!(AtspiRole(131).to_string(), "role 131");
        assert_eq!(AtspiRole(131).native_name(), "role 131");
    }

    #[rstest]
    fn a_known_role_borrows_its_native_name() {
        assert!(matches!(AtspiRole::FRAME.native_name(), Cow::Borrowed("frame")));
    }

    #[rstest]
    #[case(AtspiRole::PASSWORD_TEXT, Role::Secret)]
    #[case(AtspiRole::TERMINAL, Role::Terminal)]
    #[case(AtspiRole::DOCUMENT_WEB, Role::Document)]
    #[case(AtspiRole::SWITCH, Role::Label)]
    #[case(AtspiRole(85), Role::Container)]
    fn roles_map_to_read_roles(#[case] role: AtspiRole, #[case] expected: Role) {
        assert_eq!(role.role(), expected);
    }

    #[rstest]
    #[case::a_menu(AtspiRole::MENU, Role::Container, Role::Controls)]
    #[case::a_radio_item(AtspiRole::RADIO_MENU_ITEM, Role::Label, Role::Controls)]
    #[case::a_check_item(AtspiRole::CHECK_MENU_ITEM, Role::Label, Role::Controls)]
    #[case::a_command(AtspiRole::MENU_ITEM, Role::Controls, Role::Controls)]
    #[case::a_label(AtspiRole::LABEL, Role::Label, Role::Label)]
    fn menus_read_only_their_options(
        #[case] role: AtspiRole,
        #[case] in_menu: Role,
        #[case] outside: Role,
    ) {
        assert_eq!((role.role_in_menu(), role.role()), (in_menu, outside));
        assert_eq!(
            role.is_option(Within::Menu),
            in_menu == Role::Label && role != AtspiRole::LABEL
        );
        assert!(!role.is_option(Within::Window));
    }
}
