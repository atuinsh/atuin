//! The adjustments that single toolkits need on Linux, in one place.
//!
//! The rest of the backend follows rules that hold for every application on the accessibility
//! bus. An adjustment belongs here when it only exists because of one toolkit. Each one names
//! the toolkit, what the toolkit does differently, and how that was observed. An adjustment
//! whose arguments don't name the toolkit has a **Scope** line, which tells which applications
//! the adjustment reaches, and why that is safe.

use std::time::Duration;

use super::apps::{AppKind, Profile};
use super::bus::{Toolkit, ToolkitFamily};
use super::normalize::EMBEDDED;
use super::roles::AtspiRole;
use crate::backend::Node;
use crate::private_windows::PrivateTitles;
use crate::readiness::{Exposure, MissingAppNotes, SessionVariable};

/// Chromium, Chrome, and Electron: the time between the first request for the relations of
/// an element and the arrival of the tree of a page.
///
/// - **Differs:** these applications build the tree of a page only once a client asked any
///   element for its relations or its attributes. The backend asks every window for its
///   relations anyway, to find the document that the window shows, so this doesn't need an
///   extra call. On the first read of such a window, the request has only just gone out. The
///   window then shows its own bars and doesn't name a document yet, so nothing in the tree
///   tells that a page is still to come.
/// - **Observed:** Chromium 156 and VS Code 1.145 showed the page on the read after the
///   request, well within a second. Chromium 156's first read held 97 bytes of its bars. The
///   hook has been in Chromium's source since version 100.
/// - **Scope:** a read reports a window without a document as loading during this time after
///   the first request to its application, for every toolkit. A native window of Chromium,
///   such as its task manager, never gets a document, and a read can't tell it from a window
///   whose page is still to come. The rule costs every application one wait of a second at
///   most. A page that takes longer to build, as on a busy machine, starts its segment with a
///   snapshot of the bars alone, and the content events that follow fill the page in.
pub const PAGE_BUILD_TIME: Duration = Duration::from_secs(1);

/// Firefox and the other browsers on Gecko: whether an application with this toolkit marks
/// its private windows with [`FIREFOX_MARKER_ID`](crate::private_windows::FIREFOX_MARKER_ID).
///
/// - **Differs:** only a private window has the toolbar button that clears the private session,
///   and the button's DOM id is the element's `id` attribute. The title is translated, and the
///   button's id is the same in every language.
/// - **Observed:** Firefox ESR 153 under X11 and sway. A normal window doesn't have an element
///   with the id.
pub fn marks_private_windows(toolkit: &Toolkit) -> bool {
    toolkit.family == ToolkitFamily::Gecko
}

/// Firefox and the other browsers on Gecko: the English titles of a private window, for a
/// window without the button that clears the private session.
///
/// - **Differs:** the title is `<page> — <brand> Private Browsing`, or `<brand> Private
///   Browsing` for a window without a page. The format leaves the brand open, so that Firefox
///   Nightly, Firefox Developer Edition, and the browsers that are built on Gecko match too.
/// - **Observed:** Firefox ESR 153 under X11 and sway ended the title of a private window in
///   "— Mozilla Firefox Private Browsing". The message `browser-main-private-window-title` in
///   the browser.ftl of Firefox ESR 153 titles a window without a page.
pub fn gecko_private_titles() -> PrivateTitles {
    PrivateTitles::from_formats(["$1 Private Browsing"])
}

/// Chromium, Chrome, Edge, Brave, and Electron: whether an application with this toolkit
/// translates the title of its private windows in the resource packs beside its executable.
///
/// - **Differs:** a private window differs from a normal one only in its accessible title,
///   such as "Page - Chromium (Incognito)". The window's class and app id are the same for
///   both, and Chromium's X11 title lacks the suffix. Electron applications ship packs without
///   the title format of a private window, because Electron applications don't have private
///   windows.
/// - **Observed:** Chromium 156, Edge 158, and Brave 1.100, each in English and German, under
///   X11 and sway. The packs are `locales/<language>.pak` beside the executable.
pub fn titles_private_windows_in_packs(toolkit: &Toolkit) -> bool {
    toolkit.family == ToolkitFamily::Chromium
}

/// The variables of the session that decide whether toolkits join the accessibility bus.
/// Setup writes the setup values into the files that sessions read at login, in this order.
///
/// - **`ACCESSIBILITY_ENABLED`, Chromium, Chrome, and Electron:** these applications decide
///   once, at their start, whether to join the bus. They join when a client is connected at
///   that moment or when the variable is 1. Qt and Firefox join whenever a client connects,
///   and GTK is always there. Chromium 156, started before any client connected, stayed off
///   the bus, even after the user typed in its window. With the variable set, Chromium 156
///   joined the bus and was readable.
/// - **`NO_AT_BRIDGE`, GTK 3:** GTK 3 applications stay off the bus when the variable is 1.
///   openSUSE's sway session sets it in `/etc/sway/env`.
/// - **`GTK_A11Y`, `GNOME_ACCESSIBILITY`, and `QT_ACCESSIBILITY`:** each one keeps some
///   toolkits off the bus at its hiding value, as GTK 4's `gtkaccessibility.c`, Gecko's
///   `accessible/atk/Platform.cpp`, and Chromium's `atk_util_auralinux.cc` read them. Setup
///   reports these variables and leaves them alone.
pub const SESSION_VARIABLES: [SessionVariable; 5] = [
    SessionVariable {
        name: "ACCESSIBILITY_ENABLED",
        exposure: Exposure::Requires("1"),
        hiding_value: "0",
        unless_set: &[],
        reason: "Chromium and Electron applications share their content with accessibility tools \
                 only when such a tool was already running when the application started. Setting \
                 this variable in the session makes them always share their content.",
    },
    SessionVariable {
        name: "NO_AT_BRIDGE",
        exposure: Exposure::Overrides("0"),
        hiding_value: "1",
        unless_set: &[],
        reason: "GTK 3 applications stay off the accessibility bus while this variable is 1.",
    },
    SessionVariable {
        name: "GTK_A11Y",
        exposure: Exposure::Unless,
        hiding_value: "none",
        unless_set: &[],
        reason: "GTK 4 applications stay off the accessibility bus while this variable is none.",
    },
    SessionVariable {
        name: "GNOME_ACCESSIBILITY",
        exposure: Exposure::Unless,
        hiding_value: "0",
        unless_set: &[],
        reason: "Firefox, Chromium, and Electron applications stay off the accessibility bus \
                 while this variable is 0.",
    },
    SessionVariable {
        name: "QT_ACCESSIBILITY",
        exposure: Exposure::Unless,
        hiding_value: "0",
        unless_set: &["ACCESSIBILITY_ENABLED", "GNOME_ACCESSIBILITY"],
        reason: "Chromium and Electron applications stay off the accessibility bus while this \
                 variable is 0 and neither ACCESSIBILITY_ENABLED nor GNOME_ACCESSIBILITY is set.",
    },
];

/// An application that shows only the system's prompts, by its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptApp {
    /// The application with this name, ignoring ASCII case.
    Named(&'static str),
    /// The applications whose names start with this prefix, ignoring ASCII case.
    Prefixed(&'static str),
}

impl PromptApp {
    fn matches(self, name: &str) -> bool {
        match self {
            Self::Named(prompt) => name.eq_ignore_ascii_case(prompt),
            Self::Prefixed(prefix) => {
                name.get(..prefix.len()).is_some_and(|start| start.eq_ignore_ascii_case(prefix))
            }
        }
    }
}

/// KDE Wallet, gcr, the polkit agents, and GnuPG's pinentry: the applications that show
/// nothing but the system's prompts.
///
/// - **Differs:** their windows are ordinary dialogs, so nothing in the tree tells them apart
///   from an application's own dialogs.
/// - **Observed:** the KDE Wallet services of Plasma 5 and 6 ask for the wallet's password. In
///   a GNOME session in daily use, the first-run wizard of kwalletd6 stayed open for days.
///   gcr-prompter asks for the passwords of keyrings and keys. The polkit agents of KDE,
///   GNOME's fallback session, and MATE, such as `polkit-kde-authentication-agent-1`, and
///   `lxpolkit` of LXDE and LXQt ask for administrator passwords. GnuPG's passphrase prompts
///   are `pinentry-qt`, `pinentry-gnome3`, and others.
const PROMPT_APPS: [PromptApp; 6] = [
    PromptApp::Named("kwalletd5"),
    PromptApp::Named("kwalletd6"),
    PromptApp::Named("gcr-prompter"),
    PromptApp::Prefixed("polkit-"),
    PromptApp::Named("lxpolkit"),
    PromptApp::Prefixed("pinentry"),
];

/// Whether the application named `name` shows nothing but the system's prompts.
pub fn shows_only_prompts(name: &str) -> bool {
    PROMPT_APPS.into_iter().any(|prompt| prompt.matches(name))
}

/// Chromium, Electron, Qt, and Firefox: why these applications can be missing from a listing.
///
/// - **Differs:** Chromium and Electron decide at their start whether to join the bus, as
///   [`SESSION_VARIABLES`] describes. Qt and Firefox join only once a client turns toolkit
///   accessibility on, and Firefox acts on the change only when one of its windows takes the
///   focus.
/// - **Observed:** the table of what makes each toolkit join, in
///   `crates/atuin-desktop-capture/docs/architecture/environments.md`.
pub const MISSING_APP_NOTES: MissingAppNotes = MissingAppNotes {
    started_early: Some(
        "A browser or an Electron application that is missing started before a client was \
         connected.",
    ),
    just_woken: Some(
        "Qt applications and Firefox are visible only while capture runs. They were just asked to \
         join. If one is missing or shows few bytes, run this again while capture runs, or \
         restart that application then.",
    ),
};

/// GNOME Shell, GTK 3, and GTK 4: whether an element of the application with `profile` is
/// hidden when its height is 0, regardless of its showing state.
///
/// - **Differs:** these toolkits hide an element by setting its height to 0, and leave its
///   showing state on. Some GTK 3 lists report a width of 0 for cells that do show, so only the
///   height counts.
/// - **Scope:** only these toolkits, because another toolkit can report a height of 0 for a
///   container whose children show, and the rule hides the container with its children.
/// - **Observed:** GNOME Shell hides the warning about Caps Lock in its password prompts this
///   way. In GTK 4.18 with libadwaita 1.7, a banner that isn't revealed is a showing
///   grouping of height 0 that carries the banner's title. A revealer that is collapsed is a
///   showing grouping of height 0, whose child GTK 4 leaves out of the tree.
pub fn hides_by_height(profile: &Profile) -> bool {
    profile.kind == AppKind::Shell
        || profile
            .toolkit
            .named()
            .is_some_and(|toolkit| matches!(toolkit.family, ToolkitFamily::Gtk { major: 3 | 4 }))
}

/// GTK 4: whether `option`, a check box or a radio button in a menu, names itself only through
/// a label element of its own.
///
/// - **Differs:** an option in a GTK 4 menu leaves its name empty and has a label element
///   instead. Other toolkits name the option itself.
/// - **Observed:** the options of GTK 4 menus, whose names and texts were empty.
/// - **Scope:** every toolkit, because an option without a name or a text has nothing else to
///   read, and the label relation is the same in every toolkit.
pub fn names_option_by_label(option: &Node) -> bool {
    option.name.is_empty() && option.text.as_deref().is_none_or(|text| text.trim().is_empty())
}

/// Firefox: removes the bullet that begins the text of a list item.
///
/// - **Differs:** Firefox puts the rendered bullet and a blank into the text of the item.
///   Other toolkits keep the bullet out of the text, and the renderer writes a marker of its
///   own.
/// - **Observed:** Firefox 153 lists, where every item began with `• `.
/// - **Scope:** every list item, because the renderer writes a marker of its own for every
///   item, and a bullet at the start of the text would only repeat it.
pub fn strip_list_bullet(text: &mut String) {
    const BULLETS: [char; 4] = ['•', '◦', '▪', '‣'];
    let rest = text.trim_start_matches(BULLETS);
    if rest.len() < text.len() {
        let bullet = text.len() - rest.trim_start().len();
        text.replace_range(..bullet, "");
    }
}

/// GTK: whether an element with this role is on screen although its state lacks "showing".
///
/// - **Differs:** GTK 3 marks the items of an icon view as visible and never as showing.
///   Every other element that is on screen has both states.
/// - **Observed:** the printer list of system-config-printer, whose only content is such an
///   item.
/// - **Scope:** every icon that has the visible state, in every toolkit. No other toolkit
///   was seen to report an icon that is visible without being showing.
pub fn shows_without_the_state(role: AtspiRole) -> bool {
    role == AtspiRole::ICON
}

/// xterm.js: whether an element with `role` inside a document may be the terminal of a web
/// page or a part of one, which [`is_web_terminal`] tells from the element's classes.
///
/// - **Differs:** a native terminal has the role `terminal`. xterm.js draws its terminal on a
///   canvas and gives the terminal element no role, so browsers report a `section`. The
///   accessibility tree that xterm.js builds for screen readers sits below that element: a
///   list with an item for each line, a live region that repeats new output, and the input
///   field, which is named after the terminal's title.
/// - **Observed:** VS Code 1.146 with `"editor.accessibilitySupport": "on"`, and xterm.js
///   6.1 on a page in Chromium 156 and Firefox ESR 153.
/// - **Scope:** every element of a document that is a section, a list, or an entry, in every
///   application. The check costs one call for each such element.
pub fn may_be_web_terminal(role: AtspiRole) -> bool {
    matches!(role, AtspiRole::SECTION | AtspiRole::LIST | AtspiRole::ENTRY)
}

/// xterm.js: whether an element with the attribute `class` is the terminal of a web page or a
/// part of one, such as in VS Code's terminal panel, a cloud shell, or JupyterLab's terminal.
///
/// - **Differs:** xterm.js gives its terminal element the class `xterm`, and its parts classes
///   that start with `xterm-`, such as `xterm-accessibility-tree` for the list of rows and
///   `xterm-helper-textarea` for the input field. The live region has the class `live-region`.
///   Chromium, Electron, and Firefox copy the classes of an element into its attribute
///   `class`, separated by blanks. A browser can leave the terminal element out of its tree,
///   and the parts then hang below the page, so each part counts as a terminal too.
/// - **Observed:** VS Code 1.146 named the element `terminal xterm column-select focus`.
///   xterm.js 6.1 in Chromium 156 and Firefox ESR 153 named it
///   `terminal xterm xterm-dom-renderer-owner-1`. Firefox leaves out the element
///   `xterm-accessibility` between the terminal and its rows. Firefox ESR 153 left out the
///   terminal element of a page that copies xterm.js's elements in some reads and not in
///   others, and the live region then held the terminal's output.
/// - **Scope:** an element of another page with the class `live-region` is left out too. Such
///   an element only repeats what the page announces to screen readers.
pub fn is_web_terminal(class: &str) -> bool {
    class
        .split_ascii_whitespace()
        .any(|name| name == "xterm" || name.starts_with("xterm-") || name == "live-region")
}

/// GTK 3: whether the options in the popovers of an application with `toolkit` leave their
/// state out.
///
/// - **Differs:** the check boxes and the radio buttons of a GTK 3 popover never include
///   "checked" in their state, regardless of what the user chose. GTK 3 sends a "checked"
///   event with the value 1 about every half second for each chosen option while the popover
///   is open. GTK 3 also sends the value 1 once when an option is turned off, so the events do
///   not tell the state either. The check boxes of a GTK 3 dialog report their state, and so
///   do the check boxes of GTK 4 popovers.
/// - **Observed:** the tab width popover of gedit on GTK 3.24.49, both through `GetState` and
///   through the events, against the check boxes of gedit's preferences.
pub fn hides_popover_option_state(toolkit: &Toolkit) -> bool {
    toolkit.family == ToolkitFamily::Gtk { major: 3 }
}

/// Mutter: whether the application named `app_name` only draws the frames of the windows of
/// other applications.
///
/// - **Differs:** Mutter draws the title bar of each X11 window in a separate GTK 4 process,
///   `mutter-x11-frames`. Each frame is a top-level window of that process on the bus. The
///   name of a frame is the title of the window that the frame holds, and that title is the
///   only text in the frame. The frame and the window both announce each activation, in
///   either order. A frame also counts as an active window when its window is active.
/// - **Observed:** Geeqie 2.5 under Xwayland on GNOME 48.7. A switch to Geeqie's preferences
///   announced the preferences window and then its frame, and capture recorded the frame. A
///   switch back to the main window announced the frame first.
pub fn draws_frames(app_name: &str) -> bool {
    app_name == "mutter-x11-frames"
}

/// GTK 4: whether an application with `toolkit` shows alerts inside its windows, and announces
/// them only by adding them to a window.
///
/// - **Differs:** libadwaita shows a toast, such as "moved to trash" in Nautilus, as an
///   element with the role alert inside the window, and announces it only by adding it to
///   the window. A toast closes by itself after a few seconds, so a read of the window that
///   waits for the window to settle often misses it. Browsers and GTK 3 tree views add
///   elements all the time, one for each cell, so asking for the role of each added element
///   is too slow there. GTK 3 doesn't have toasts.
/// - **Observed:** Nautilus 48 on GTK 4.18, where the toast is a child of a grouping and the
///   only event is `children-changed:add` with the alert as the child.
pub fn shows_alerts_inside_windows(toolkit: &Toolkit) -> bool {
    toolkit.family == ToolkitFamily::Gtk { major: 4 }
}

/// GTK 4: whether an application with `toolkit` leaves the elements without a role out of the
/// tree, together with everything inside them.
///
/// - **Differs:** GTK 4 leaves an element with the role "none" or "presentation" out of the
///   children of its parent, and out of every listing. The element still answers on the bus,
///   with the role "invalid". Its children answer too, and their parents lead back to the
///   tree. libadwaita gives the role "presentation" to its spin row, so the title and the value
///   of every spin row are missing. Screen readers meet such a row only when it takes the focus.
///   GTK 4's cache lists such an element only after the element took the focus. The first
///   field of a dialog takes the focus when the dialog opens, and a row further down stays
///   missing until the user tabs or clicks into it.
/// - **Observed:** GNOME Calculator 48 on GTK 4.18 and libadwaita 1.7. The "Number of
///   Decimals" row of its preferences was absent from the tree, and the spin button inside it
///   answered with its name and its value. GTK 4.20 and libadwaita 1.8 behave the same.
pub fn leaves_roleless_elements_out(toolkit: &Toolkit) -> bool {
    toolkit.family == ToolkitFamily::Gtk { major: 4 }
}

/// GTK 4: drops the text of `element`, an element that GTK 4 leaves out of the tree, when an
/// element inside it repeats that text. See [`leaves_roleless_elements_out`].
///
/// - **Differs:** libadwaita's spin row repeats the text of the spin button inside it as its
///   own text. An element with text shows the text in place of its children, so the row read
///   as its value alone, without its title.
/// - **Observed:** the "Number of Decimals" row of GNOME Calculator 48's preferences.
/// - **Scope:** only the elements that answer with the role "invalid", which GTK 4 gives the
///   elements that it leaves out of the tree.
pub fn drop_repeated_text(element: &mut Node) {
    fn repeats(node: &Node, text: &str) -> bool {
        node.children.iter().any(|child| {
            child.text.as_deref().is_some_and(|own| own.trim() == text) || repeats(child, text)
        })
    }

    if let Some(text) = element.text.as_deref().map(str::trim)
        && !text.is_empty()
        && repeats(element, text)
    {
        element.text = None;
    }
}

/// Chromium, Chrome, and Electron: whether a showing child of the window with `role` covers
/// the page.
///
/// - **Differs:** when a page shows an `alert()`, Chromium puts the alert in place of the
///   window's content, and the window keeps naming the page as its document. The read would
///   take the missing page for one that is still being built, and capture would wait for the
///   page as long as the alert stayed open.
/// - **Observed:** Chromium on GNOME 48, with a page that called `alert()`.
/// - **Scope:** every toolkit, because the rule only keeps a read from waiting for a page
///   when a dialog of the window shows.
pub const fn dialog_covers_page(role: AtspiRole) -> bool {
    matches!(role, AtspiRole::DIALOG | AtspiRole::ALERT)
}

/// GNOME Shell: whether the application named `app_name` is the desktop shell.
///
/// - **Differs:** GNOME Shell draws the overview, the top bar and its menus, and the run
///   dialog on one window of its own, "Main stage". That window becomes active whenever the
///   user opens the overview to switch applications, and its text is then the overview's
///   search or the calendar of the top bar. Capture recorded each visit as a window named
///   "Main stage", which didn't tell an agent anything.
/// - **Observed:** GNOME Shell 48.7 on Wayland, through the overview, the calendar menu, and
///   Looking Glass.
pub fn is_desktop_shell(app_name: &str) -> bool {
    app_name == "gnome-shell"
}

/// Chromium, Chrome, and Electron: whether the document at `url` is part of the browser's
/// own interface.
///
/// - **Differs:** Chromium draws some of its interface as web pages in windows of their own,
///   with URLs such as `chrome://omnibox-popup.top-chrome/`. Such a window holds the
///   suggestions for what the user types into the address bar, and never a page. Other
///   browsers draw their interface with toolkit elements, which capture skips by role.
/// - **Observed:** Chromium 156 on GNOME 48. Each URL typed into the address bar left a
///   background window without a title, named only by the URL of the suggestions.
/// - **Scope:** the documents whose URL has the `chrome://` scheme and a host that ends in
///   `.top-chrome`, which only Chromium's own interface uses.
pub fn is_browser_ui(url: &str) -> bool {
    url.strip_prefix("chrome://").is_some_and(|rest| {
        let host = rest.split('/').next().unwrap_or_default();
        host.ends_with(".top-chrome")
    })
}

/// Chromium, Chrome, and Electron: one child of an element whose text repeats the element's
/// plain runs of text.
///
/// - **Differs:** Chromium leaves out a text node of blanks whose neighbours in the page are
///   comments, although the page shows the blank. `<span>a</span><!----> <!----><span>b</span>`
///   reads as `ab`. Frameworks such as the one that draws ChatGPT put comments around the
///   blank between every two words, so whole answers read as one word. The positions of the
///   characters on each side of a seam tell where the page shows a blank. See [`seams`].
/// - **Observed:** Chromium 156 on chatgpt.com, and on a page with each kind of seam. Firefox
///   153 keeps the blanks.
/// - **Scope:** every element whose text repeats the runs of its children, as Chromium's
///   elements do. A blank goes back only where the positions of the characters leave room for
///   one, so text that kept its blanks stays as it is.
#[derive(Debug, Clone, Copy)]
pub enum Piece<'a> {
    /// A plain run, which holds this many characters of the parent's text.
    Run {
        characters: usize,
    },
    /// A child that the parent's text holds as one marker, with the child's own text. A child
    /// without text, such as an empty button, lies within the seam between its neighbours.
    Embedded {
        text: Option<&'a str>,
    },
}

/// Whether `text`, the text of an element with this many children, may lack a blank that the
/// page shows between two of the children. See [`Piece`].
///
/// Words that the page sets apart with blanks usually come with a run of blanks between every
/// two of them, so the text holds at least half as many words as the element has children.
/// Text that lost those runs holds fewer. This check doesn't make any calls, so it saves a call
/// for every run of text that [`seams`] would make. For example, a CI log can have thousands of
/// lines.
pub fn may_lack_blanks(text: &str, children: usize) -> bool {
    children >= 2 && text.split_whitespace().count() * 2 < children + 1
}

/// Two children that meet without a blank between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seam {
    /// The index of the child before the seam.
    pub before: usize,
    /// The index of the child after the seam. Only children without text lie between the two.
    pub after: usize,
    /// The offset of the last character of the child before the seam, in that child's text.
    /// The child after the seam starts at offset 0 of its own text.
    pub last: usize,
    /// Where a blank would go in the parent's text, as an offset in characters.
    pub offset: usize,
}

/// Finds the places in `text`, the text of an element with the children `pieces`, where two
/// children meet without a blank.
///
/// Returns `None` when `text` does not consist of exactly the pieces.
pub fn seams(text: &str, pieces: &[Piece<'_>]) -> Option<Vec<Seam>> {
    /// The characters at the edges of one child, where they can stand next to a blank.
    struct Edges {
        index: usize,
        first: Option<char>,
        last: Option<char>,
        /// The number of characters of the child's own text.
        characters: usize,
    }

    fn word_character(c: char) -> Option<char> {
        (!c.is_whitespace() && c != EMBEDDED).then_some(c)
    }

    let mut rest = text;
    let mut offset = 0;
    let mut previous: Option<Edges> = None;
    let mut seams = Vec::new();
    for (index, piece) in pieces.iter().enumerate() {
        let (own, length) = match *piece {
            Piece::Run { characters } => {
                let end = rest.char_indices().nth(characters).map_or(rest.len(), |(at, _)| at);
                let (run, after) = rest.split_at(end);
                if run.chars().count() != characters || run.contains(EMBEDDED) {
                    return None;
                }
                rest = after;
                (Some(run), characters)
            }
            Piece::Embedded { text } => {
                rest = rest.strip_prefix(EMBEDDED)?;
                (text, 1)
            }
        };
        let Some(own) = own else {
            offset += length;
            continue;
        };
        let edges = Edges {
            index,
            first: own.chars().next().and_then(word_character),
            last: own.chars().next_back().and_then(word_character),
            characters: own.chars().count(),
        };
        if let Some(before) = &previous
            && before.last.is_some()
            && edges.first.is_some()
        {
            seams.push(Seam {
                before: before.index,
                after: index,
                last: before.characters - 1,
                offset,
            });
        }
        offset += length;
        previous = Some(edges);
    }
    rest.is_empty().then_some(seams)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::vs_code("terminal xterm column-select focus", true)]
    #[case::a_page("terminal xterm xterm-dom-renderer-owner-1", true)]
    #[case::alone("xterm", true)]
    #[case::the_list_of_rows("xterm-accessibility-tree", true)]
    #[case::the_input_field("xterm-helper-textarea", true)]
    #[case::the_live_region("live-region", true)]
    #[case::another_word("terminal-xterm-host", false)]
    #[case::no_class("", false)]
    fn xterm_js_marks_its_terminal_by_class(#[case] class: &str, #[case] expected: bool) {
        assert_eq!(is_web_terminal(class), expected);
    }

    #[rstest]
    #[case::kwallet_in_another_case("KWalletD6", true)]
    #[case::kwallet_5("kwalletd5", true)]
    #[case::keyring_prompt("gcr-prompter", true)]
    #[case::kde_polkit_agent("polkit-kde-authentication-agent-1", true)]
    #[case::lxde_polkit_agent("lxpolkit", true)]
    #[case::pinentry("pinentry-qt", true)]
    #[case::editor("kate", false)]
    #[case::kwallet_manager("kwalletmanager5", false)]
    #[case::shorter_than_a_prefix("pin", false)]
    fn prompt_apps_match_by_name(#[case] name: &str, #[case] expected: bool) {
        assert_eq!(shows_only_prompts(name), expected);
    }

    /// Setup writes the variables that have a setup value into the login files, so the list
    /// keeps the content of those files.
    #[rstest]
    fn setup_sets_two_variables_in_order() {
        let set: Vec<_> = SESSION_VARIABLES
            .iter()
            .filter_map(|variable| Some((variable.name, variable.setup_value()?)))
            .collect();
        assert_eq!(set, [("ACCESSIBILITY_ENABLED", "1"), ("NO_AT_BRIDGE", "0")]);
    }

    #[rstest]
    #[case::page("Release notes — Mozilla Firefox Private Browsing", true)]
    #[case::without_a_page("Mozilla Firefox Private Browsing", true)]
    #[case::nightly("Release notes — Firefox Nightly Private Browsing", true)]
    #[case::normal("#infra - Chat — Mozilla Firefox", false)]
    fn gecko_titles(#[case] title: &str, #[case] expected: bool) {
        assert_eq!(gecko_private_titles().matches(title), expected);
    }

    #[rstest]
    #[case::omnibox("chrome://omnibox-popup.top-chrome/", true)]
    #[case::tab_search("chrome://tab-search.top-chrome/", true)]
    #[case::settings("chrome://settings/", false)]
    #[case::page("https://example.com/x.top-chrome/", false)]
    fn browser_ui_is_known_by_its_url(#[case] url: &str, #[case] expected: bool) {
        assert_eq!(is_browser_ui(url), expected);
    }

    fn element(text: Option<&str>, children: Vec<Node>) -> Node {
        Node {
            text: text.map(str::to_owned),
            children,
            ..Node::default()
        }
    }

    #[rstest]
    #[case::repeated_deeper(Some("2"), Some("2"), None)]
    #[case::repeated_with_blanks(Some(" 2 "), Some("2"), None)]
    #[case::own_text(Some("Precision"), Some("2"), Some("Precision"))]
    fn unlisted_elements_lose_only_the_text_that_their_children_repeat(
        #[case] text: Option<&str>,
        #[case] inner: Option<&str>,
        #[case] expected: Option<&str>,
    ) {
        let button = element(inner, Vec::new());
        let title = element(Some("Number of Decimals"), Vec::new());
        let boxed = element(None, vec![button]);
        let mut row = element(text, vec![title, boxed]);
        drop_repeated_text(&mut row);
        assert_eq!(row.text.as_deref(), expected);
    }

    #[rstest]
    #[case::bullet("• first point", "first point")]
    #[case::nested_bullet("◦ second level", "second level")]
    #[case::only_a_bullet("• ", "")]
    #[case::no_bullet("first point", "first point")]
    #[case::indented_text("  first point", "  first point")]
    fn bullets_leave_list_items(#[case] text: &str, #[case] expected: &str) {
        let mut text = text.to_owned();
        strip_list_bullet(&mut text);
        assert_eq!(text, expected);
    }

    #[rstest]
    // `Yes!` `Atuin` `is` `good`, where `Atuin` is a link.
    #[case::runs_and_a_link(
        "Yes!\u{fffc}isgood",
        &[run(4), embedded(Some("Atuin")), run(2), run(4)],
        Some(&[seam(0, 1, 3, 4), seam(1, 2, 4, 5), seam(2, 3, 1, 7)][..])
    )]
    #[case::blanks_already_there("a b", &[run(1), run(1), run(1)], Some(&[][..]))]
    #[case::blank_inside_a_run("a bc", &[run(2), run(2)], Some(&[][..]))]
    #[case::child_without_text(
        "a\u{fffc}b",
        &[run(1), embedded(None), run(1)],
        Some(&[seam(0, 2, 0, 2)][..])
    )]
    #[case::empty_child("a\u{fffc}b", &[run(1), embedded(Some("")), run(1)], Some(&[][..]))]
    #[case::child_that_starts_with_a_marker(
        "a\u{fffc}",
        &[run(1), embedded(Some("\u{fffc}x"))],
        Some(&[][..])
    )]
    #[case::text_left_over("abc", &[run(1), run(1)], None)]
    #[case::too_little_text("ab", &[run(1), run(2)], None)]
    #[case::marker_where_a_run_belongs("a\u{fffc}", &[run(1), run(1)], None)]
    #[case::wide_characters("日本語", &[run(2), run(1)], Some(&[seam(0, 1, 1, 2)][..]))]
    fn seams_are_where_children_touch(
        #[case] text: &str,
        #[case] pieces: &[Piece<'static>],
        #[case] expected: Option<&[Seam]>,
    ) {
        assert_eq!(seams(text, pieces).as_deref(), expected);
    }

    #[rstest]
    // `<span>with</span><!----> <!----><span>comments</span>`: the blank runs are gone.
    #[case::lost_blanks("C3 withcommentsstatic", 4, true)]
    #[case::glued_answer("Yes!\u{fffc}\u{fffc}isanexcellenttool", 6, true)]
    #[case::table_cell("Directoryfiltering", 2, true)]
    #[case::blank_runs_between_words("a b c", 5, false)]
    // A line of a CI log: a line number, a blank, and the line.
    #[case::log_line("12 Compiling crate-12 v0.1.12", 3, false)]
    #[case::single_child("abc", 1, false)]
    fn lost_blanks_show_in_the_word_count(
        #[case] text: &str,
        #[case] children: usize,
        #[case] expected: bool,
    ) {
        assert_eq!(may_lack_blanks(text, children), expected);
    }

    const fn run(characters: usize) -> Piece<'static> {
        Piece::Run { characters }
    }

    const fn embedded(text: Option<&'static str>) -> Piece<'static> {
        Piece::Embedded { text }
    }

    const fn seam(before: usize, after: usize, last: usize, offset: usize) -> Seam {
        Seam {
            before,
            after,
            last,
            offset,
        }
    }
}
