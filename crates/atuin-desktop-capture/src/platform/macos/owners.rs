//! The applications that own windows, which the watcher observes for new windows, sheets, and
//! changes of content.

use std::collections::HashMap;
use std::time::Instant;

use super::desktop::{BundleIds, WindowOwners, append_regular_apps, is_terminal_bundle};
use super::element::Pid;
use super::observer::{AxNotification, AxObserver, Delivery, ObserveRetry, observe};
use crate::backend::Screen;

/// The notifications that capture needs from every application with a window.
///
/// They report new windows and sheets, dialogs inside a window, and changes of content. Capture
/// reads changes of content in background windows too.
const OWNER_NOTIFICATIONS: [AxNotification; 6] = [
    AxNotification::WindowCreated,
    AxNotification::SheetCreated,
    AxNotification::FocusedUiElementChanged,
    AxNotification::ValueChanged,
    AxNotification::LoadComplete,
    AxNotification::LayoutChanged,
];

/// The notifications that capture needs from a terminal, whose content it never reads: the
/// windows and sheets that can hold a dialog.
const TERMINAL_NOTIFICATIONS: [AxNotification; 2] =
    [AxNotification::WindowCreated, AxNotification::SheetCreated];

/// What [`Owners`] needs to observe an application.
pub struct Observing<'a> {
    pub delivery: Delivery,
    pub retry: &'a mut ObserveRetry,
    pub bundles: &'a BundleIds,
}

/// The frontmost application now, and before the look that is under way.
#[derive(Debug, Clone, Copy)]
pub struct Fronts {
    pub now: Option<Pid>,
    pub before: Option<Pid>,
}

/// The applications that own windows, and an observer of each one.
#[derive(Default)]
pub struct Owners {
    /// The owners of the on-screen windows where a dialog can appear, as of the last scan.
    /// Nothing owns such a window while the screen is locked.
    windows: WindowOwners,
    /// The applications in `windows` that were announced, or that owned a window when capture
    /// started.
    announced: Vec<Pid>,
    /// The applications that the last scan followed: the owners of windows, and every other
    /// regular application. The vec is kept to reuse its allocation.
    followed: Vec<Pid>,
    /// An observer of [`OWNER_NOTIFICATIONS`] for every application in `followed`, and for the
    /// frontmost application.
    observers: HashMap<Pid, AxObserver>,
    /// Whether `windows` was filled once. Only the applications that come to own a window after
    /// the first scan are announced.
    started: bool,
}

impl Owners {
    /// Lists the owners of the windows on screen.
    pub fn scan(&mut self, screen: Screen, bundles: &BundleIds) {
        match screen {
            Screen::Locked => self.windows.clear(),
            Screen::Unlocked => self.windows.collect(bundles),
        }
    }

    /// Observes new windows, sheets, and changes of content in every application that owns a window
    /// on screen, and in every regular application.
    ///
    /// Returns the applications that came to own a window on screen since the last scan.
    ///
    /// A regular application is observed because its windows can all be minimized. The
    /// observers of the frontmost applications in `fronts` stay.
    pub fn follow(
        &mut self,
        screen: Screen,
        fronts: Fronts,
        mut observing: Observing<'_>,
    ) -> &[Pid] {
        let mut followed = std::mem::take(&mut self.followed);
        followed.clone_from(&self.windows.readable);
        // Nothing is followed while the screen is locked.
        if screen == Screen::Unlocked {
            append_regular_apps(&mut followed);
        }
        let kept = |pid: &Pid| followed.contains(pid) || fronts.now == Some(*pid);
        self.observers.retain(|pid, _| kept(pid) || fronts.before == Some(*pid));
        observing.retry.retain(kept);
        // Every scan looks up the bundle ids of the processes that own windows, so those stay.
        observing.bundles.retain(|pid| kept(pid) || self.windows.owns_a_window(*pid));
        let now = Instant::now();
        for &pid in &followed {
            if self.observers.contains_key(&pid) || observing.retry.backs_off(pid, now) {
                continue;
            }
            // An application that is not ready is announced once a later scan observes it.
            if let Some(observer) = observe_owner(pid, &mut observing, now) {
                self.observers.insert(pid, observer);
            }
        }
        self.followed = followed;
        self.announce()
    }

    /// Notes the applications that own a window on screen and have an observer, and returns
    /// the ones that are new since the last scan.
    ///
    /// An application that shows only a dialog comes to own a window with the dialog. Such an
    /// application can have an observer already, because it became frontmost before its window
    /// showed. For example, UserNotificationCenter becomes frontmost before its alerts show.
    fn announce(&mut self) -> &[Pid] {
        self.announced.retain(|pid| self.windows.readable.contains(pid));
        let known = self.announced.len();
        for &pid in &self.windows.readable {
            if self.observers.contains_key(&pid) && !self.announced.contains(&pid) {
                self.announced.push(pid);
            }
        }
        if std::mem::replace(&mut self.started, true) {
            &self.announced[known..]
        } else {
            &[]
        }
    }

    /// Observes `app`, which just became frontmost. The frontmost application can change
    /// content before a scan finds its first window.
    pub fn observe_front(&mut self, app: Pid, mut observing: Observing<'_>, now: Instant) {
        if !self.observers.contains_key(&app)
            && let Some(observer) = observe_owner(app, &mut observing, now)
        {
            self.observers.insert(app, observer);
        }
    }

    /// Drops every observer.
    pub fn stop_observing(&mut self) {
        self.observers.clear();
    }

    /// Drops every observer, and forgets which applications were announced. The next scan
    /// starts over, as the first one does.
    pub fn start_over(&mut self) {
        self.stop_observing();
        self.announced.clear();
        self.started = false;
    }
}

/// Observes the notifications that capture needs from `app`, which owns a window.
fn observe_owner(app: Pid, observing: &mut Observing<'_>, now: Instant) -> Option<AxObserver> {
    let notifications: &[AxNotification] = if observing.bundles.matches(app, is_terminal_bundle) {
        &TERMINAL_NOTIFICATIONS
    } else {
        &OWNER_NOTIFICATIONS
    };
    let delivery = observing.delivery;
    observing.retry.attempt(app, now, || observe(app, notifications, delivery))
}
