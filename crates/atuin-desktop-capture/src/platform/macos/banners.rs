//! Notification banners, which the Notification Center process draws for every application.
//!
//! - **Differs:** a banner isn't a window of the application that sent it. Notification Center
//!   (`com.apple.notificationcenterui`) draws every banner and alert as an element of one
//!   full-screen window at window layer 23, which is above the layers where dialogs appear.
//!   The process doesn't create a window for a banner, and it announces a banner only with
//!   `AXLayoutChanged` on the banner element, twice while the banner slides in. A banner
//!   disappears after 5 seconds, and an alert stays until the user dismisses it.
//! - **Attribution:** the name of the application that sent a notification appears only in the
//!   banner's `AXDescription`. The title, the subtitle, and the body are children of their own.
//!   - On macOS 15 and later, the description reads "app, title, subtitle, body". The texts
//!     appear in it exactly as their children hold them, line breaks included.
//!   - On macOS 13 and 14, the description holds only the name.
//!   - A notification without a title shows the name of its sender in the place of the title,
//!     and a notification that the system sends about an application, such as the request to
//!     allow notifications, doesn't name a sender. On macOS 15 and later, the description of either
//!     kind consists of the texts alone. On macOS 13 and 14, the description equals the title.
//!     Such a banner doesn't tell its sender.
//!
//!   The texts can carry secrets. Capture scrubs the name as it scrubs the texts, and cuts the
//!   name short, but a scrubber misses some secrets. The name therefore comes only from a
//!   description that has one of the forms above, and it never equals one of the texts. A
//!   banner whose description has any other form is skipped, and a banner that doesn't tell
//!   its sender is handed out without a name.
//! - **Localized names:** the name is the sender's name in the language of the system, such as
//!   ターミナル for Terminal on a Japanese system. The notice of a banner therefore names the
//!   running application of that name as its process, so that capture can tell a terminal by
//!   its bundle id. A banner of a terminal that no longer runs is told by the localized names
//!   of the installed terminals.
//! - **Observed:** macOS 13.7.4, 14.8.7, 15.7.7, and 27.2.1, in English, with banners from
//!   `UNUserNotificationCenter`, from `display notification` in `osascript`, and from the
//!   system.

use std::collections::VecDeque;

use atuin_common::string::StrExt as _;
use core_foundation::string::CFString;
use objc2_foundation::NSProcessInfo;
use tracing::debug;

use super::desktop::{localized_terminal_names, running_app_named};
use super::element::{Element, str_of};
use super::observer::AxNotification;
use crate::backend::Notice;

pub const BUNDLE_ID: &str = "com.apple.notificationcenterui";

/// The notifications that announce a banner.
pub const NOTIFICATIONS: [AxNotification; 1] = [AxNotification::LayoutChanged];

/// The subroles of a banner, which disappears by itself, and of an alert, which stays.
const SUBROLES: [&str; 2] = ["AXNotificationCenterBanner", "AXNotificationCenterAlert"];

/// How many banners are remembered to recognize a banner that is announced again.
const REMEMBERED: usize = 32;

/// The banners that were already handed out.
pub struct Seen {
    banners: VecDeque<String>,
    form: DescriptionForm,
    /// The localized names of the installed terminals, once a banner needed them.
    terminal_names: Option<Vec<String>>,
}

impl Default for Seen {
    fn default() -> Self {
        Self {
            banners: VecDeque::new(),
            form: DescriptionForm::of_this_system(),
            terminal_names: None,
        }
    }
}

impl Seen {
    /// Reads the notification that `element` shows, when `element` is a banner that has not
    /// been read yet.
    ///
    /// The notice doesn't name an application when the banner doesn't tell its sender.
    ///
    /// Returns `None` for a banner that lacks its description or its title, or whose
    /// description doesn't match its texts. A banner can show either while it slides in.
    /// Notification Center announces a banner twice while it slides in, and the second
    /// announcement reads such a banner again.
    pub fn read(&mut self, element: &Element) -> Option<Notice> {
        let subrole = element.attribute("AXSubrole")?.downcast::<CFString>()?;
        if !SUBROLES.contains(&&*str_of(&subrole)) {
            return None;
        }
        // Every banner has an identifier of its own.
        let id = element.string("AXIdentifier")?;
        if self.banners.contains(&id) {
            return None;
        }

        let mut parts = Parts::default();
        for child in element.elements("AXChildren") {
            let Some(identifier) =
                child.attribute("AXIdentifier").and_then(|value| value.downcast::<CFString>())
            else {
                continue;
            };
            let slot = match &*str_of(&identifier) {
                "title" => &mut parts.title,
                "subtitle" => &mut parts.subtitle,
                "body" => &mut parts.body,
                _ => continue,
            };
            *slot = child.string("AXValue").filter(|text| !text.is_empty());
        }
        let Some(mut app) = element.string("AXDescription").filter(|_| parts.title.is_some())
        else {
            debug!(
                subrole = &*str_of(&subrole),
                title = parts.title.is_some(),
                "a banner is skipped, because its description or its title is missing"
            );
            return None;
        };
        match sender(&app, &parts, self.form) {
            Sender::Named(name) => app.truncate(name.len()),
            Sender::Untold => {
                debug!(subrole = &*str_of(&subrole), "a banner doesn't tell its sender");
                app.clear();
            }
            Sender::Unrecognized => {
                debug!(
                    subrole = &*str_of(&subrole),
                    form = ?self.form,
                    "a banner is skipped, because its description doesn't match its texts"
                );
                return None;
            }
        }

        if self.banners.len() == REMEMBERED {
            self.banners.pop_front();
        }
        self.banners.push_back(id);
        let pid = match running_app_named(&app) {
            Some(pid) => u32::try_from(pid.raw()).ok(),
            None if !app.is_empty() && self.is_terminal_name(&app) => {
                debug!("a banner of a terminal that no longer runs is skipped");
                return None;
            }
            None => None,
        };
        let Parts {
            title,
            subtitle,
            body,
        } = parts;
        // The subtitle leads the body, because a notice doesn't have a place of its own for it.
        let body = match (subtitle, body) {
            (Some(mut subtitle), Some(body)) => {
                subtitle.push('\n');
                subtitle.push_str(&body);
                subtitle
            }
            (subtitle, body) => subtitle.or(body).unwrap_or_default(),
        };
        Some(Notice {
            app,
            pid,
            app_id: String::new(),
            title: title.unwrap_or_default(),
            body,
        })
    }
}

impl Seen {
    /// Whether `name` is the localized name of an installed terminal.
    fn is_terminal_name(&mut self, name: &str) -> bool {
        self.terminal_names
            .get_or_insert_with(localized_terminal_names)
            .iter()
            .any(|terminal| terminal.eq_ignoring_case(name))
    }
}

/// How Notification Center describes a banner, which depends on the version of macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DescriptionForm {
    /// macOS 13 and 14 describe a banner by the name of its sender alone.
    Name,
    /// macOS 15 and later describe a banner by the name of its sender, followed by its texts.
    NameAndTexts,
}

impl DescriptionForm {
    fn of_this_system() -> Self {
        if NSProcessInfo::processInfo().operatingSystemVersion().majorVersion < 15 {
            Self::Name
        } else {
            Self::NameAndTexts
        }
    }
}

/// What the description of a banner tells about the banner's sender.
#[derive(Debug, PartialEq, Eq)]
enum Sender<'a> {
    Named(&'a str),
    /// The description has a known form that doesn't name a sender.
    Untold,
    /// The description doesn't have a known form. It may hold the texts in a way that isn't
    /// known, so no part of it can serve as the name.
    Unrecognized,
}

/// The texts of a banner, each of which is a child element.
#[derive(Default)]
struct Parts {
    title: Option<String>,
    subtitle: Option<String>,
    body: Option<String>,
}

impl Parts {
    /// The texts in the order in which the banner shows them.
    fn texts(&self) -> impl DoubleEndedIterator<Item = &str> {
        [&self.title, &self.subtitle, &self.body].into_iter().flatten().map(String::as_str)
    }
}

/// What `description` tells about the sender of the banner that shows `parts`, on a system
/// that describes banners in `form`.
///
/// A description names the sender in one of two forms:
/// - On every version of macOS, the description lists the texts in order, each one after
///   ", ". What precedes the texts is the name, unless it equals one of the texts. A
///   description of the texts alone doesn't name a sender.
/// - On macOS 13 and 14, the description is the name, unless it holds one of the texts. A
///   description that equals a text, such as a title in the place of the name, doesn't name a
///   sender. A description that holds ", " isn't taken, because that separator belongs to the
///   form that lists the texts. The texts are looked for regardless of whitespace, because a
///   description might break the lines of a text in other places.
fn sender<'a>(description: &'a str, parts: &Parts, form: DescriptionForm) -> Sender<'a> {
    if let Some(preceding) = before_texts(description, parts) {
        return match preceding.strip_suffix(", ") {
            _ if preceding.is_empty() => Sender::Untold,
            Some(name) if parts.texts().any(|text| text == name) => Sender::Untold,
            Some(name) if !name.is_empty() => Sender::Named(name),
            _ => Sender::Unrecognized,
        };
    }
    if form == DescriptionForm::NameAndTexts || description.is_empty() {
        return Sender::Unrecognized;
    }
    if parts.texts().any(|text| text == description) {
        Sender::Untold
    } else if description.contains(", ") {
        Sender::Unrecognized
    } else if parts.texts().any(|text| description.contains_ignoring_whitespace(text)) {
        Sender::Untold
    } else {
        Sender::Named(description)
    }
}

/// What precedes the texts of `parts` in `description`, when `description` ends with the texts
/// in order, with ", " between each two of them.
fn before_texts<'a>(description: &'a str, parts: &Parts) -> Option<&'a str> {
    let mut rest = description;
    for (index, text) in parts.texts().rev().enumerate() {
        if index > 0 {
            rest = rest.strip_suffix(", ")?;
        }
        rest = rest.strip_suffix(text)?;
    }
    Some(rest)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::DescriptionForm::{Name, NameAndTexts};
    use super::{DescriptionForm, Parts, Sender, sender};

    /// The descriptions and the texts of banners as macOS showed them, and descriptions whose
    /// form isn't known.
    #[rstest]
    #[case::every_part(
        "Probe Notifier, Deploy, finished, Probe subtitle, All 3 checks passed",
        Some("Deploy, finished"),
        Some("Probe subtitle"),
        Some("All 3 checks passed"),
        Sender::Named("Probe Notifier")
    )]
    #[case::a_title_and_a_body(
        "Script Editor, Title PLOVER, Body PLOVER",
        Some("Title PLOVER"),
        None,
        Some("Body PLOVER"),
        Sender::Named("Script Editor")
    )]
    // A name that equals a text could be the text of a banner without a sender.
    #[case::texts_that_repeat_the_name(
        "Script Editor, Script Editor, Script Editor",
        Some("Script Editor"),
        None,
        Some("Script Editor"),
        Sender::Untold
    )]
    #[case::a_system_sender(
        "Login Items, App Background Activity, “atuin” can run in the background.",
        Some("App Background Activity"),
        None,
        Some("“atuin” can run in the background."),
        Sender::Named("Login Items")
    )]
    #[case::texts_with_line_breaks(
        "Script Editor, Title one\ntitle two, Body one\nbody two",
        Some("Title one\ntitle two"),
        None,
        Some("Body one\nbody two"),
        Sender::Named("Script Editor")
    )]
    #[case::texts_with_tabs_quotes_and_returns(
        "Script Editor, Quote \"x\", Sub\nline, Tab\tbody\rline",
        Some("Quote \"x\""),
        Some("Sub\nline"),
        Some("Tab\tbody\rline"),
        Sender::Named("Script Editor")
    )]
    // macOS 15 and 27 put the sender's name in the place of a missing title, so the name looks
    // like a title.
    #[case::no_title(
        "Script Editor, Your code is 445566",
        Some("Script Editor"),
        None,
        Some("Your code is 445566"),
        Sender::Untold
    )]
    #[case::no_title_with_a_subtitle(
        "Script Editor, Sub WREN, Body WREN",
        Some("Script Editor"),
        Some("Sub WREN"),
        Some("Body WREN"),
        Sender::Untold
    )]
    #[case::no_sender(
        "Software Update Available, To install this update, you’ll need to free up storage.",
        Some("Software Update Available"),
        None,
        Some("To install this update, you’ll need to free up storage."),
        Sender::Untold
    )]
    #[case::no_sender_on_a_permission_request(
        "“Brave Browser” Notifications, Notifications may include alerts, sounds, and icon badges.",
        Some("“Brave Browser” Notifications"),
        None,
        Some("Notifications may include alerts, sounds, and icon badges."),
        Sender::Untold
    )]
    // A banner that slides in can lack a text that its description already holds.
    #[case::a_subtitle_that_is_missing(
        "Bank, Sign-in, New device, Your code is 123456",
        Some("Sign-in"),
        None,
        Some("Your code is 123456"),
        Sender::Unrecognized
    )]
    #[case::a_body_that_is_missing(
        "Bank, Sign-in, Your code is 123456",
        Some("Sign-in"),
        None,
        None,
        Sender::Unrecognized
    )]
    #[case::the_name_alone(
        "Script Editor",
        Some("Title CURLEW"),
        Some("Sub CURLEW"),
        Some("Body CURLEW"),
        Sender::Unrecognized
    )]
    #[case::unexpected_order(
        "Mail, body, title",
        Some("title"),
        None,
        Some("body"),
        Sender::Unrecognized
    )]
    #[case::truncated_body(
        "Messages, Your code, 1234",
        Some("Your code"),
        None,
        Some("123456"),
        Sender::Unrecognized
    )]
    #[case::nothing_before_the_texts(
        ", Your code",
        Some("Your code"),
        None,
        None,
        Sender::Unrecognized
    )]
    #[case::a_text_inside_the_description(
        "Code 123456 here",
        Some("123456"),
        None,
        None,
        Sender::Unrecognized
    )]
    #[case::an_empty_description("", Some("Title"), None, Some("Body"), Sender::Unrecognized)]
    fn the_sender_comes_from_a_description_with_the_texts(
        #[case] description: &str,
        #[case] title: Option<&str>,
        #[case] subtitle: Option<&str>,
        #[case] body: Option<&str>,
        #[case] expected: Sender,
    ) {
        assert_eq!(sender(description, &parts(title, subtitle, body), NameAndTexts), expected);
    }

    /// The descriptions and the texts of banners as macOS 13 and 14 showed them, and
    /// descriptions whose form isn't known.
    #[rstest]
    #[case::only_the_name(
        "Script Editor",
        Some("Title CURLEW"),
        Some("Sub CURLEW"),
        Some("Body CURLEW"),
        Sender::Named("Script Editor")
    )]
    #[case::only_the_name_of_a_system_sender(
        "Login Items",
        Some("Background Items Added"),
        None,
        Some("“GoogleUpdater” added items that can run in the background."),
        Sender::Named("Login Items")
    )]
    #[case::a_short_text("Calendar", Some("Lunch"), None, Some("a"), Sender::Untold)]
    #[case::only_the_name_without_a_title(
        "Script Editor",
        Some("Script Editor"),
        None,
        Some("Your code is 445566"),
        Sender::Untold
    )]
    #[case::only_the_title_without_a_sender(
        "“AskPerm” Notifications",
        Some("“AskPerm” Notifications"),
        None,
        Some("Notifications may include alerts, sounds, and icon badges."),
        Sender::Untold
    )]
    // The form of later versions is recognized too.
    #[case::the_name_and_the_texts(
        "Script Editor, Title PLOVER, Body PLOVER",
        Some("Title PLOVER"),
        None,
        Some("Body PLOVER"),
        Sender::Named("Script Editor")
    )]
    #[case::the_texts_alone(
        "Script Editor, Your code is 445566",
        Some("Script Editor"),
        None,
        Some("Your code is 445566"),
        Sender::Untold
    )]
    #[case::texts_that_the_description_renders_differently(
        "Bank, Your code is 123456, Use it now",
        Some("Your code\nis 123456"),
        None,
        Some("Use it\nnow"),
        Sender::Unrecognized
    )]
    #[case::a_title_that_the_description_breaks_differently(
        "Your code is 123456",
        Some("Your code\nis 123456"),
        None,
        Some("Use it now"),
        Sender::Untold
    )]
    #[case::a_body_that_is_missing(
        "Bank, Sign-in, Your code is 123456",
        Some("Sign-in"),
        None,
        None,
        Sender::Unrecognized
    )]
    #[case::a_text_inside_the_description(
        "Code 123456 here",
        Some("123456"),
        None,
        None,
        Sender::Untold
    )]
    #[case::an_empty_description("", Some("Title"), None, Some("Body"), Sender::Unrecognized)]
    fn the_sender_comes_from_a_description_with_the_name(
        #[case] description: &str,
        #[case] title: Option<&str>,
        #[case] subtitle: Option<&str>,
        #[case] body: Option<&str>,
        #[case] expected: Sender,
    ) {
        assert_eq!(sender(description, &parts(title, subtitle, body), Name), expected);
    }

    /// A banner's texts can carry secrets that the scrubber can miss, so the code in the texts
    /// never reaches the name in either form.
    #[rstest]
    fn a_text_never_becomes_the_name(
        #[values(Name, NameAndTexts)] form: DescriptionForm,
        #[values("Your code is 123456", "Your code\nis 123456", "Your code is\n123456\n")]
        text: &str,
        #[values(
            "Your code is 123456",
            "Bank, Your code is 123456",
            "Your code is 123456, Your code is 123456",
            "Bank, Your code is 123456, Use it now"
        )]
        description: &str,
    ) {
        for parts in [
            parts(Some(text), None, None),
            parts(Some("Sign-in"), None, Some(text)),
            parts(Some("Sign-in"), Some(text), Some("Use it now")),
        ] {
            if let Sender::Named(name) = sender(description, &parts, form) {
                assert!(!name.contains("123456"), "{name:?} holds the code");
            }
        }
    }

    fn parts(title: Option<&str>, subtitle: Option<&str>, body: Option<&str>) -> Parts {
        Parts {
            title: title.map(str::to_owned),
            subtitle: subtitle.map(str::to_owned),
            body: body.map(str::to_owned),
        }
    }
}
