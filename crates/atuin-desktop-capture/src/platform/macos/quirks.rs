//! The adjustments that single applications need on macOS, in one place.
//!
//! The rest of the backend follows rules that hold for every application that answers the
//! AX API. An adjustment belongs here when it only exists because of one application. Each
//! one names the application, what the application does differently, and how that was
//! observed.

use std::path::{Path, PathBuf};
use std::time::Duration;

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::string::CFString;
use objc2_foundation::{NSBundle, NSString};
use tracing::debug;

use super::element::{Element, str_of};
use crate::private_windows::{PrivateTitles, Translations};

/// How long an application needs to build its tree once [`wake`] asked for it.
///
/// - **Observed:** Chrome and VS Code on macOS 26.6 were readable within 2 seconds. Safari on
///   macOS 27.2 showed its page about a second after the first request.
pub const WAKE_TIME: Duration = Duration::from_secs(2);

/// How long after the first request [`wake`] asks an application again, if the application
/// is still frontmost.
///
/// - **Differs:** VS Code ignores the request while it launches, so the request that comes as
///   the application first activates builds the window's chrome but not its editor. A
///   request a few seconds later builds the editor too. Every other application has built
///   its tree by then, and ignores the second request.
/// - **Observed:** VS Code with `"editor.accessibilitySupport": "auto"` on macOS 13.7, 14.8,
///   and 15.7. Without the second request, the editor stayed empty for as long as the window
///   stayed active, and its text appeared only after the user switched away and back.
pub const WAKE_AGAIN: Duration = Duration::from_secs(4);

/// Chrome and Electron: asks `app` to build its accessibility tree.
///
/// - **Differs:** native applications always answer. Electron builds its tree when a client
///   sets `AXManualAccessibility`, which Electron documents. Chrome builds its tree once a
///   client reads the role of the application element.
/// - **Observed:** Chrome and VS Code on macOS 26.6.
/// - **Scope:** both calls go to every application, because they are harmless on the others.
///
/// Numbers: also sets `AXEnhancedUserInterface` when `bundle_id` is Numbers.
///
/// - **Differs:** Numbers exposes the sheets of a spreadsheet only while a client has set
///   `AXEnhancedUserInterface`, as VoiceOver does. Without it, the canvas is an empty scroll
///   area. The attribute also changes how some applications animate their windows, so it goes
///   to Numbers alone.
/// - **Observed:** Numbers 15 on macOS 27.2. The call reports an error, and the canvas lists
///   its tables right after it.
pub fn wake(app: &Element, bundle_id: Option<&str>) {
    app.set_true("AXManualAccessibility");
    let _ = app.attribute("AXRole");
    if bundle_id == Some("com.apple.Numbers") {
        app.set_true("AXEnhancedUserInterface");
    }
}

/// Numbers: returns the text of a cell or of a column's heading from its title. Returns `None`
/// for a reference tab, which shows a column's or a row's letter instead of content.
///
/// - **Differs:** a cell of Numbers has its text in its title, followed by its kind for a
///   heading cell, as in "q1, Column header cell". The table's header holds the letters above
///   the columns, as buttons titled "Column A reference tab".
/// - **Observed:** Numbers 15 on macOS 27.2, in English.
/// - **Scope:** the reader applies this to the cells of every application. The suffixes are
///   too specific to occur in the cells of another application, and no attribute of a cell
///   tells its kind: the role, subrole, and role description of every cell are the same.
pub fn cell_title(mut title: String) -> Option<String> {
    let reference_tab = (title.starts_with("Column ") || title.starts_with("Row "))
        && title.ends_with(" reference tab");
    if reference_tab {
        return None;
    }
    let mut text = title.as_str();
    for kind in [", Row header cell", ", Column header cell"] {
        text = text.strip_suffix(kind).unwrap_or(text);
    }
    title.truncate(text.len());
    Some(title)
}

/// The system agents that show nothing but prompts, by bundle id. The listing of applications
/// includes their windows above the modal panels, and capture records a prompt as a dialog.
///
/// - **Differs:** their windows are standard windows, so nothing in the AX API marks them as
///   dialogs. SecurityAgent shows its password prompt at window layer 1000, above every layer
///   where an application's dialog can appear.
/// - **Observed:** on macOS 27.2: SecurityAgent's prompt for administrator access, which
///   `do shell script … with administrator privileges` shows, and universalAccessAuthWarn's
///   "Device Control and Data Access" prompt, which a process without the Accessibility
///   permission causes.
/// - **Scope:** capture leaves out universalAccessAuthWarn, as [`is_permission_alert`]
///   describes, so only `doctor` and `apps` list its prompt. Capture records SecurityAgent's
///   prompts.
const PROMPT_AGENTS: [&str; 2] = ["com.apple.SecurityAgent", PERMISSION_ALERT];

/// The bundle id of the agent that shows the alert that says that a program "would like to
/// control this computer".
pub const PERMISSION_ALERT: &str = "com.apple.accessibility.universalAccessAuthWarn";

/// Whether the application with this bundle id shows nothing but prompts, which capture
/// records as dialogs.
pub fn shows_only_prompts(bundle_id: &str) -> bool {
    PROMPT_AGENTS.contains(&bundle_id)
}

/// GnuPG's pinentry: whether the application named `name` is a prompt for a passphrase, such
/// as `pinentry-mac`.
///
/// - **Differs:** the prompt is an ordinary window of an ordinary application, which GnuPG
///   starts for each passphrase that it needs.
/// - **Observed:** GPG Suite and Homebrew name the program `pinentry-mac`.
/// - **Scope:** only the suggestion of the applications to capture leaves it out. Capture
///   records its prompts as windows.
pub fn is_passphrase_prompt(name: &str) -> bool {
    name.get(.."pinentry".len()).is_some_and(|start| start.eq_ignore_ascii_case("pinentry"))
}

/// universalAccessAuthWarn: whether the application with this bundle id shows the alert about
/// the Accessibility permission, which capture leaves out.
///
/// - **Differs:** the alert stays open after the user grants the permission, unlike the prompts
///   of other agents, so capture would keep a segment of an alert that nobody looks at.
/// - **Observed:** after the helper asked for the permission, the alert's window is titled
///   "Accessibility Access" on macOS 13, 14, and 15, and "Device Control and Data Access" on
///   27.2. With `apps = "all"`, capture on 27.2 recorded the alert as a segment.
/// - **Closing:** the agent quits when it is asked to, as other applications do, so setup asks
///   it to quit once the user grants the permission. See
///   [`close_permission_alert`](super::permission::close_permission_alert).
pub fn is_permission_alert(bundle_id: &str) -> bool {
    bundle_id == PERMISSION_ALERT
}

/// loginwindow: whether the application with this bundle id is the one that draws the lock
/// screen, which capture leaves out.
///
/// - **Differs:** the lock screen is an ordinary window of loginwindow, and the AX API reads
///   it while the screen is locked, unlike the windows of every other application. It shows
///   the date, the time, and the full names of the Mac's users.
/// - **Observed:** on macOS 15.7, loginwindow became the active application about a millisecond
///   before the session was marked as locked. Capture followed its window, titled "Login", and
///   stored the lock screen.
/// - **Scope:** capture doesn't read anything while the screen is locked. Leaving loginwindow
///   out also covers the moment before capture learns of the lock.
pub fn is_login_window(bundle_id: &str) -> bool {
    bundle_id == "com.apple.loginwindow"
}

/// SecurityAgent: whether `title` is a placeholder rather than a title.
///
/// - **Differs:** SecurityAgent titles every prompt "Untitled".
/// - **Observed:** the prompt for administrator access on macOS 27.2.
pub fn is_placeholder_title(bundle_id: &str, title: &str) -> bool {
    bundle_id == "com.apple.SecurityAgent" && title == "Untitled"
}

/// Finder: whether an image that `title` names is the icon of a file, whose title is the
/// file's name.
///
/// - **Differs:** Finder's icon view names its files only on their icons. Other applications
///   name an image for what it shows, if at all.
/// - **Observed:** Finder's icon view, whose file names were otherwise missing from the read.
/// - **Scope:** the reader applies this to the images of every application, because no
///   attribute tells Finder's icons apart. Other applications rarely give an image a title,
///   and a titled image reads well as a label.
pub fn names_file_on_icon(title: Option<&str>) -> bool {
    title.is_some_and(|title| !title.is_empty())
}

/// xterm.js: whether an element whose `AXDOMClassList` holds `classes` is the terminal of a
/// web page or a part of one, such as in VS Code's terminal panel, a cloud shell, or
/// JupyterLab's terminal.
///
/// - **Differs:** no AX role marks a terminal, so capture knows terminal applications by
///   their names. xterm.js gives its terminal element the class `xterm`, and its parts classes
///   that start with `xterm-`, such as `xterm-accessibility-tree` for the list of rows and
///   `xterm-helper-textarea` for the input field. The live region, which repeats new output,
///   has the class `live-region`. Browsers list the classes of an element in
///   `AXDOMClassList`. A browser can leave the terminal element out of its tree, and the parts
///   then hang below the page, so each part counts as a terminal too.
/// - **Observed:** VS Code 1.146 with `"editor.accessibilitySupport": "on"`, whose rows and
///   output were stored before this rule.
/// - **Scope:** every element of every application. Only web content has the attribute. An
///   element of another page with the class `live-region` is left out too. Such an element
///   only repeats what the page announces to screen readers.
pub fn is_web_terminal(classes: &CFType) -> bool {
    classes.downcast::<CFArray>().is_some_and(|classes| {
        classes.iter().any(|class| {
            // SAFETY: the array holds live CoreFoundation objects, and wrapping retains the item.
            let class = unsafe { CFType::wrap_under_get_rule(*class) };
            class.downcast::<CFString>().is_some_and(|class| {
                let class = str_of(&class);
                *class == *"xterm" || class.starts_with("xterm-") || *class == *"live-region"
            })
        })
    })
}

/// Chrome: whether `window` is a window that only holds the alert of a page.
///
/// - **Differs:** Chrome shows a page's alert in a window of its own, which isn't a standard
///   window and whose only child is the dialog. Safari and Firefox show it as a dialog group
///   inside the page's window. The alert's window doesn't name the page's window in any
///   attribute, but the page's window holds the same dialog element below its first group, so
///   the private-window check finds the page's window through the dialog. An alert that no
///   other window holds this way fails the private-window check on every read.
/// - **Observed:** the alerts of pages in Chrome, Edge, and Brave on macOS 27.2.1, in normal
///   and private windows.
/// - **Scope:** the dialog rules apply this to the windows of every application, because a
///   window that holds only an element with the subrole of a page's alert holds such an alert
///   in any browser.
pub fn holds_page_alert(window: &Element) -> bool {
    let has_subrole = |element: &Element, wanted: fn(&str) -> bool| {
        element
            .attribute("AXSubrole")
            .and_then(|subrole| subrole.downcast::<CFString>())
            .is_some_and(|subrole| wanted(&str_of(&subrole)))
    };
    let mut children = window.elements("AXChildren");
    let (Some(child), None) = (children.next(), children.next()) else {
        return false;
    };
    !has_subrole(window, |subrole| subrole == "AXStandardWindow")
        && has_subrole(&child, is_page_alert)
}

/// Whether an element with `subrole` is a page's alert, as Chrome shows it in a window of its
/// own.
pub fn is_page_alert(subrole: &str) -> bool {
    matches!(subrole, "AXApplicationDialog" | "AXApplicationAlertDialog")
}

/// The library that the bundle of every application on Gecko holds, below the bundle's root.
const GECKO_LIBRARY: &str = "Contents/MacOS/XUL";

/// Firefox and the other browsers on Gecko: whether the application bundle at `bundle` holds Gecko.
///
/// Gecko's browsers mark their private windows with
/// [`FIREFOX_MARKER_ID`](crate::private_windows::FIREFOX_MARKER_ID).
///
/// - **Differs:** only a private window has the toolbar button that clears the private session,
///   and the button's DOM id is its `AXDOMIdentifier`. The title is translated, and the
///   button's id is the same in every language. Gecko is a library in the bundle of every
///   application on Gecko, such as Firefox, LibreWolf, Zen, and Thunderbird.
/// - **Observed:** Firefox 160 on macOS 27.2. A normal window doesn't have an element with the
///   id.
pub fn holds_gecko(bundle: &Path) -> bool {
    bundle.join(GECKO_LIBRARY).is_file()
}

/// Firefox: the English titles of a private window, for a window without the button that
/// clears the private session.
///
/// - **Differs:** the title is `<page> — Private Browsing`, without the brand that Firefox
///   writes into the title on Linux.
/// - **Observed:** Firefox 160 on macOS 27.2.
pub fn gecko_private_titles() -> PrivateTitles {
    PrivateTitles::from_formats(["$1 — Private Browsing"])
}

/// Safari: whether the application with this bundle id is Safari.
///
/// - **Differs:** a private window differs from a normal one only in its title. Both kinds of
///   window have the identifier `SafariWindow?IsSecure=false&UUID=…`.
/// - **Observed:** Safari on macOS 27.2, whose private window was titled
///   "#infra - Chat, Private Browsing".
pub fn is_safari(bundle_id: &str) -> bool {
    matches!(bundle_id, "com.apple.Safari" | "com.apple.SafariTechnologyPreview")
}

/// Safari: whether the application bundle at `bundle` is Safari's, for an application whose
/// bundle id can't be looked up. See [`is_safari`].
pub fn is_safari_bundle(bundle: &Path) -> bool {
    bundle
        .file_name()
        .is_some_and(|name| name == "Safari.app" || name == "Safari Technology Preview.app")
}

/// The framework that holds Safari's translations.
const SAFARI_FRAMEWORK: &str = "/System/Library/PrivateFrameworks/Safari.framework";

/// The keys of Safari's formats of a private window's title. The key is also the English
/// text. The first one holds the page's title, and the second one titles a window without a
/// page.
const SAFARI_TITLE_KEYS: [&str; 2] = ["%@, Private Browsing", "Private Browsing (window title)"];

/// Safari: the English titles of its private windows, for a Safari whose translations can't
/// be read. The English text of the second key of [`SAFARI_TITLE_KEYS`] is "Private Browsing".
pub fn safari_english_titles() -> PrivateTitles {
    PrivateTitles::from_formats(["%@, Private Browsing", "Private Browsing"])
}

/// Safari: the titles of its private windows, in every language of Safari's framework.
///
/// Returns `None` when the framework's languages can't be listed. The translations are partial when
/// a language's table can't be opened.
///
/// - **Differs:** Safari's framework translates the formats in its `Localizable` table, such
///   as `%@, „Privates Surfen“` in German. Safari can run in another language than the system
///   and the helper, so every language counts.
/// - **Observed:** the framework on macOS 27.2 holds both keys in every language that it
///   ships.
pub fn safari_private_titles() -> Option<Translations> {
    let mut titles = safari_english_titles();
    let mut complete = true;
    let mut resources = PathBuf::from(SAFARI_FRAMEWORK);
    resources.push("Resources");
    let table = NSString::from_str("Localizable");
    let keys = SAFARI_TITLE_KEYS.map(NSString::from_str);
    let languages = std::fs::read_dir(&resources).ok()?.filter_map(|entry| {
        let path = entry.ok()?.path();
        path.extension().is_some_and(|extension| extension == "lproj").then_some(path)
    });
    for language in languages {
        // A language's directory opens as a bundle whose table is that language's alone.
        let Some(bundle) =
            language.to_str().and_then(|path| NSBundle::bundleWithPath(&NSString::from_str(path)))
        else {
            debug!(language = %language.display(), "a language of Safari can't be opened");
            complete = false;
            continue;
        };
        titles.extend(keys.iter().filter_map(|key| {
            let text = bundle.localizedStringForKey_value_table(key, None, Some(&table));
            // A missing key reads as the key itself.
            (*text != **key).then(|| text.to_string())
        }));
    }
    Some(if complete {
        Translations::Complete(titles)
    } else {
        Translations::Partial(titles)
    })
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::gpg_suite("pinentry-mac", true)]
    #[case::in_another_case("Pinentry-Mac", true)]
    #[case::application("Preview", false)]
    #[case::shorter_than_the_prefix("pin", false)]
    fn passphrase_prompts_match_by_name(#[case] name: &str, #[case] expected: bool) {
        assert_eq!(is_passphrase_prompt(name), expected);
    }

    #[rstest]
    #[case::plain("1350", Some("1350"))]
    #[case::column_heading("q1, Column header cell", Some("q1"))]
    #[case::corner("region, Column header cell, Row header cell", Some("region"))]
    #[case::reference_tab("Column A reference tab", None)]
    #[case::text_that_mentions_a_tab(
        "Column notes, reference tab list",
        Some("Column notes, reference tab list")
    )]
    fn numbers_cells_lose_their_kind(#[case] title: &str, #[case] expected: Option<&str>) {
        assert_eq!(cell_title(title.to_owned()).as_deref(), expected);
    }

    #[rstest]
    #[case::vs_code(&["terminal", "xterm", "column-select", "focus"], true)]
    #[case::the_list_of_rows(&["xterm-accessibility-tree"], true)]
    #[case::the_input_field(&["xterm-helper-textarea"], true)]
    #[case::the_live_region(&["live-region"], true)]
    #[case::another_word(&["terminal-xterm-host"], false)]
    #[case::no_class(&[], false)]
    fn xterm_js_marks_its_terminal_by_class(#[case] classes: &[&str], #[case] expected: bool) {
        let classes: Vec<CFString> = classes.iter().map(|class| CFString::new(class)).collect();
        let classes = CFArray::from_CFTypes(&classes);
        assert_eq!(is_web_terminal(&classes.as_CFType()), expected);
    }

    #[rstest]
    fn an_element_without_classes_is_no_terminal() {
        assert!(!is_web_terminal(&CFString::new("xterm").as_CFType()));
    }

    /// Safari's translations come from the framework that every macOS install ships.
    #[rstest]
    #[case::english("Notes, Private Browsing")]
    #[case::german("Notizen, „Privates Surfen“")]
    #[case::without_a_page("Private Browsing")]
    fn safari_titles_cover_every_language(#[case] title: &str) {
        let Some(Translations::Complete(titles)) = safari_private_titles() else {
            panic!("every language of the framework reads");
        };
        assert!(titles.matches(title));
    }
}
