//! Reads the dialogs and notification windows that appear, as soon as they settle.

use std::time::{Duration, SystemTime};

use tokio::time::Instant;
use tracing::debug;

use super::context::Context;
use super::pacing::{Retries, Retry};
use super::record::{ContentHash, Dialog, Notification, Record, WindowId};
use super::scrubber::Found;
use crate::backend::{Backend, WindowClass};
use crate::render::{first_line, is_blank};

/// How long a dialog with the same content in the same window counts as the same appearance.
///
/// A toolkit can announce one window in more than one way, and each announcement leads to a
/// read.
const REPEAT_WINDOW: Duration = Duration::from_secs(1);

/// The most characters of a dialog's text that stand in for its missing title.
const MAX_TITLE_CHARS: usize = 100;

/// How many times in a row a dialog whose read fails or finds nothing is read again.
///
/// An application is often busy right when it shows an error, and a dialog that closes before
/// the user comes to it doesn't get another read.
const MAX_DIALOG_RETRIES: u8 = 3;

/// The retries of a dialog's read.
type DialogRetries = Retries<MAX_DIALOG_RETRIES>;

/// How capture learned of a window that may be a dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Announcement {
    Opened,
    /// The window became active, and the activation found the window's class unless the class was
    /// unknown.
    ///
    /// Some toolkits announce a new window only this way, and a user can come back to a dialog that
    /// capture has already read.
    Activated(Option<WindowClass>),
}

/// What a window that `Dialogs` hands out is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Appearance {
    Dialog,
    /// A notification that the platform reports as a window.
    Notification,
}

impl Appearance {
    /// How a window of `class` appears, when capture hands such a window out.
    const fn of(class: WindowClass) -> Option<Self> {
        match class {
            WindowClass::Dialog => Some(Self::Dialog),
            WindowClass::Notification => Some(Self::Notification),
            WindowClass::Regular | WindowClass::Popup | WindowClass::NoticeWindow => None,
        }
    }
}

/// What appeared on the desktop.
enum Arrival<B: Backend> {
    Window(Expected<B>),
    /// An application whose windows appeared with it.
    App(B::App),
}

/// A window that may be a dialog, and waits to be read.
struct Expected<B: Backend> {
    window: B::Window,
    announcement: Announcement,
    retries: DialogRetries,
}

/// Something that appeared and is looked at once it settles.
struct Appeared<B: Backend> {
    arrival: Arrival<B>,
    due: Instant,
}

/// A dialog that was handed out a moment ago.
struct RecentDialog {
    window: WindowId,
    hash: ContentHash,
    at: Instant,
}

/// The windows and applications that appeared, and the dialogs that were handed out lately.
pub struct Dialogs<B: Backend> {
    /// The windows and applications that wait to be looked at.
    appeared: Vec<Appeared<B>>,
    /// The dialogs that were handed out within the last [`REPEAT_WINDOW`].
    recent: Vec<RecentDialog>,
}

impl<B: Backend> Dialogs<B> {
    pub const fn new() -> Self {
        Self {
            appeared: Vec::new(),
            recent: Vec::new(),
        }
    }

    /// Looks at `window` once it settles, unless the window waits already.
    pub fn expect_window(
        &mut self,
        window: B::Window,
        announcement: Announcement,
        settle: Duration,
    ) {
        if !self.awaits(&window) {
            self.expect(
                Arrival::Window(Expected {
                    window,
                    announcement,
                    retries: DialogRetries::default(),
                }),
                Instant::now() + settle,
            );
        }
    }

    /// Looks at the windows of `app` once the application settles.
    pub fn expect_app(&mut self, app: B::App, settle: Duration) {
        self.expect(Arrival::App(app), Instant::now() + settle);
    }

    /// When the next arrival is due to be looked at.
    pub fn due(&self) -> Option<Instant> {
        self.appeared.iter().map(|appeared| appeared.due).min()
    }

    /// Looks at the arrival that is due first: reads a window when it is a dialog, and lists
    /// the windows of an application.
    pub async fn look(&mut self, ctx: &mut Context<B>) {
        let Some((first, _)) = self.appeared.iter().enumerate().min_by_key(|(_, a)| a.due) else {
            return;
        };
        match self.appeared.swap_remove(first).arrival {
            Arrival::Window(expected) => self.read(ctx, expected).await,
            Arrival::App(app) => {
                for window in ctx.backend.windows(&app).await.unwrap_or_default() {
                    self.expect_window(window, Announcement::Opened, Duration::ZERO);
                }
            }
        }
    }

    fn expect(&mut self, arrival: Arrival<B>, due: Instant) {
        self.appeared.push(Appeared { arrival, due });
    }

    /// Whether `window` already waits to be looked at.
    fn awaits(&self, window: &B::Window) -> bool {
        self.appeared.iter().any(|appeared| {
            matches!(&appeared.arrival, Arrival::Window(expected) if expected.window == *window)
        })
    }

    /// Hands out the expected window as a dialog or a notification when it is one.
    ///
    /// An activation doesn't add a record for a window that capture already handed out. A
    /// window whose class is unknown is classified again once it settles.
    async fn read(&mut self, ctx: &mut Context<B>, expected: Expected<B>) {
        let window = &expected.window;
        let class = match expected.announcement {
            Announcement::Activated(_) if ctx.windows.was_handed_out(window) => return,
            Announcement::Activated(Some(class)) => class,
            Announcement::Activated(None) | Announcement::Opened => {
                match ctx.backend.window_class(window).await {
                    Ok(class) => class,
                    Err(error) => {
                        debug!(%error, "the class of a window that may be a dialog is unknown");
                        self.retry(ctx, expected);
                        return;
                    }
                }
            }
        };
        let Some(appearance) = Appearance::of(class) else {
            return;
        };
        let app = ctx.backend.app_of(window);
        if !ctx.allows_app(&app).await {
            return;
        }
        let read = ctx.read(window).await;
        let reading = match read.value {
            Ok(reading) => reading,
            Err(error) => {
                debug!(?app, %error, "reading a dialog failed");
                self.retry(ctx, expected);
                return;
            }
        };
        if !ctx.config.captures(reading.kind) {
            return;
        }
        if is_blank(&reading.root) {
            debug!(?app, "the dialog has nothing readable yet");
            self.retry(ctx, expected);
            return;
        }
        let found = Found::new(reading.root, read.elapsed, reading.stats);
        let Some(record) = self.record(ctx, window, appearance, found).await else {
            return;
        };
        ctx.records.push_back(record);
    }

    /// Reads the expected window again once it settles, unless its retries are used up.
    fn retry(&mut self, ctx: &Context<B>, mut expected: Expected<B>) {
        if expected.retries.try_again() == Retry::Stop {
            debug!("the dialog is given up");
            return;
        }
        let due = Instant::now() + ctx.config.timing.dialog_settle;
        self.expect(Arrival::Window(expected), due);
    }

    /// The record of a dialog or a notification that `window` shows, unless the window showed
    /// the same content a moment ago.
    async fn record(
        &mut self,
        ctx: &mut Context<B>,
        window: &B::Window,
        appearance: Appearance,
        found: Found,
    ) -> Option<Record> {
        let raw_title = ctx.backend.window_title(window).await.unwrap_or_default();
        let title = ctx.scrubber.title(raw_title);
        let cleaned = ctx.scrubber.clean(found);
        // Alerts, toasts, and GNOME Shell's prompts have no title, and their first line
        // says what they are about.
        let title = if title.trim().is_empty() {
            first_line(&cleaned.root, MAX_TITLE_CHARS)
        } else {
            title
        };
        let content = cleaned.into_content();
        if self.is_repeat(ctx.windows.id(window), content.hash()) {
            debug!("the dialog was announced again");
            return None;
        }
        let app = ctx.backend.app_of(window);
        let app = ctx.allowed_app_name(&app).await?.to_owned();
        // A later activation of the window doesn't hand out the window again.
        let window = ctx.windows.handed_out_id(window);
        let at = SystemTime::now();
        Some(match appearance {
            Appearance::Dialog => Record::Dialog(Dialog {
                window,
                app,
                title,
                at,
                content,
            }),
            Appearance::Notification => Record::Notification(Notification {
                app,
                title,
                at,
                content,
            }),
        })
    }

    /// Whether the same content of the same window was handed out a moment ago. A dialog that
    /// isn't a repeat is remembered for the next check.
    fn is_repeat(&mut self, window: WindowId, hash: ContentHash) -> bool {
        let now = Instant::now();
        self.recent.retain(|recent| now.duration_since(recent.at) < REPEAT_WINDOW);
        if self.recent.iter().any(|recent| recent.window == window && recent.hash == hash) {
            return true;
        }
        self.recent.push(RecentDialog {
            window,
            hash,
            at: now,
        });
        false
    }
}
