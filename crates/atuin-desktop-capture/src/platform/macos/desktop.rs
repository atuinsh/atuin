//! The applications and windows on the desktop, as the window server and the workspace list
//! them.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::Arc;

use atuin_common::string::StrExt as _;
use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace};
use objc2_foundation::{NSFileManager, NSString};
use parking_lot::Mutex;

use super::element::{Element, Pid, is_text};
use super::{ffi, quirks};
use crate::backend::Screen;

/// The longest that one AX call may take. A hung application doesn't answer any call.
const MESSAGING_TIMEOUT_SECS: f32 = 1.0;

/// The window layer of the ordinary windows of every application.
const NORMAL_LAYER: i32 = 0;
/// The highest window layer where a dialog can appear: the layer of modal panels, where the
/// system agents show their prompts. The layers in between hold floating panels.
const MODAL_PANEL_LAYER: i32 = 8;

/// The bundle id of an application, such as `com.apple.Safari`.
///
/// The id stays the same in every language, unlike the application's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleId(Arc<str>);

impl BundleId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The bundle ids of running applications. A lookup that succeeds is kept until the process
/// exits.
#[derive(Default)]
pub struct BundleIds(Mutex<HashMap<Pid, BundleId>>);

impl BundleIds {
    /// Returns the bundle id of the application with the process id `pid`.
    ///
    /// A failed lookup is tried again on the next call, so that a lookup that failed for a
    /// moment is not remembered for the life of the process.
    pub fn get(&self, pid: Pid) -> Option<BundleId> {
        if let Some(bundle_id) = self.0.lock().get(&pid) {
            return Some(bundle_id.clone());
        }
        let bundle_id = BundleId(bundle_id(pid)?.into());
        self.0.lock().insert(pid, bundle_id.clone());
        Some(bundle_id)
    }

    /// Whether the application `pid` has a bundle id, and `matches` accepts the id.
    pub fn matches(&self, pid: Pid, matches: impl FnOnce(&str) -> bool) -> bool {
        self.get(pid).is_some_and(|bundle_id| matches(bundle_id.as_str()))
    }

    /// Forgets the bundle id of `pid`, whose process exited. A new process can reuse the id.
    pub fn forget(&self, pid: Pid) {
        self.0.lock().remove(&pid);
    }

    /// Forgets the bundle ids of the processes that `kept` rejects.
    pub fn retain(&self, kept: impl Fn(&Pid) -> bool) {
        self.0.lock().retain(|pid, _| kept(pid));
    }
}

/// The processes that own windows on screen, as the last scan found them. The lists keep their
/// allocations from one scan to the next.
#[derive(Default)]
pub struct WindowOwners {
    /// The applications whose windows capture reads, frontmost first.
    pub readable: Vec<Pid>,
    /// The processes that own a window above the modal panels and aren't system agents.
    ///
    /// Such a process can own many of those windows. For example, the menu bar's process owns one
    /// for each menu bar item.
    others: Vec<Pid>,
    /// The `others` of the scan before.
    ///
    /// A process that isn't a system agent stays that way, so the next scan doesn't look up its
    /// bundle id again. A process without a bundle id would otherwise be looked up on every scan,
    /// because a lookup that fails isn't kept.
    earlier_others: Vec<Pid>,
}

impl WindowOwners {
    /// Lists the owners of the windows on screen.
    ///
    /// Capture reads normal windows, modal panels, and the prompts of system agents, which can
    /// sit above every other layer.
    pub fn collect(&mut self, bundles: &BundleIds) {
        let Self {
            readable,
            others,
            earlier_others,
        } = self;
        readable.clear();
        std::mem::swap(others, earlier_others);
        others.clear();
        visit_windows(|pid, layer| {
            if readable.contains(&pid) {
                return ControlFlow::Continue(());
            }
            let read = if (NORMAL_LAYER..=MODAL_PANEL_LAYER).contains(&layer) {
                true
            } else if layer > MODAL_PANEL_LAYER && !others.contains(&pid) {
                let agent = !earlier_others.contains(&pid)
                    && bundles.matches(pid, quirks::shows_only_prompts);
                if !agent {
                    others.push(pid);
                }
                agent
            } else {
                false
            };
            if read {
                readable.push(pid);
            }
            ControlFlow::Continue(())
        });
    }

    /// Forgets every owner, as when the screen locks.
    pub fn clear(&mut self) {
        self.readable.clear();
        self.others.clear();
        self.earlier_others.clear();
    }

    /// Whether the process `pid` owns a window on screen, whether capture reads the window or
    /// not.
    pub fn owns_a_window(&self, pid: Pid) -> bool {
        self.readable.contains(&pid) || self.others.contains(&pid)
    }
}

/// Appends to `apps` the applications that a person works with, which are the ones with an
/// icon in the Dock, unless `apps` holds them already.
///
/// An application whose windows are all minimized doesn't own a window on screen, and its
/// windows are still worth reading.
pub fn append_regular_apps(apps: &mut Vec<Pid>) {
    let running = NSWorkspace::sharedWorkspace().runningApplications();
    for index in 0..running.count() {
        let app = running.objectAtIndex(index);
        let pid = Pid::new(app.processIdentifier());
        if app.activationPolicy() == NSApplicationActivationPolicy::Regular && !apps.contains(&pid)
        {
            apps.push(pid);
        }
    }
}

/// Lists the applications whose windows capture may read: the ones that own a window on
/// screen, frontmost first, and then the other regular applications.
pub fn readable_apps(bundles: &BundleIds) -> Vec<Pid> {
    let mut owners = WindowOwners::default();
    owners.collect(bundles);
    let mut apps = owners.readable;
    append_regular_apps(&mut apps);
    apps
}

/// The names of terminal emulators, as they name themselves in English.
const TERMINAL_APPS: &[&str] = &[
    "Alacritty",
    "contour",
    "cool-retro-term",
    "Ghostty",
    "Hyper",
    "iTerm2",
    "kitty",
    "rio",
    "Tabby",
    "Terminal",
    "Warp",
    "WezTerm",
];

/// The bundle ids of terminal emulators. An application's name follows the language of the
/// system, and its bundle id stays the same.
const TERMINAL_BUNDLES: &[&str] = &[
    "co.zeit.hyper",
    "com.apple.Terminal",
    "com.github.wez.wezterm",
    "com.googlecode.iterm2",
    "com.mitchellh.ghostty",
    "com.raphaelamorim.rio",
    "dev.warp.Warp-Stable",
    "net.kovidgoyal.kitty",
    "org.alacritty",
    "org.tabby",
];

/// Whether `name` is the name of a terminal emulator.
pub fn is_terminal_name(name: &str) -> bool {
    TERMINAL_APPS.iter().any(|terminal| terminal.eq_ignore_ascii_case(name))
}

/// The running application whose name in the language of the system is `name`, ignoring
/// case. A regular application, which has an icon in the Dock, comes before the others.
pub fn running_app_named(name: &str) -> Option<Pid> {
    if name.is_empty() {
        return None;
    }
    let running = NSWorkspace::sharedWorkspace().runningApplications();
    let mut found = None;
    for index in 0..running.count() {
        let app = running.objectAtIndex(index);
        if !app
            .localizedName()
            .is_some_and(|localized| localized.to_string().eq_ignoring_case(name))
        {
            continue;
        }
        let pid = Pid::new(app.processIdentifier());
        if app.activationPolicy() == NSApplicationActivationPolicy::Regular {
            return Some(pid);
        }
        found.get_or_insert(pid);
    }
    found
}

/// The names of the installed terminal emulators in the language of the system, as Finder
/// shows them.
pub fn localized_terminal_names() -> Vec<String> {
    let workspace = NSWorkspace::sharedWorkspace();
    let files = NSFileManager::defaultManager();
    TERMINAL_BUNDLES
        .iter()
        .filter_map(|bundle_id| {
            let url =
                workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))?;
            let path = url.path()?;
            let name = files.displayNameAtPath(&path).to_string();
            Some(match name.strip_suffix(".app") {
                Some(stem) => stem.to_owned(),
                None => name,
            })
        })
        .collect()
}

/// Returns the bundle id of the application with the process id `pid`.
fn bundle_id(pid: Pid) -> Option<String> {
    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid.raw())?;
    Some(app.bundleIdentifier()?.to_string())
}

/// Whether the application with this bundle id is a terminal emulator.
pub fn is_terminal_bundle(bundle_id: &str) -> bool {
    TERMINAL_BUNDLES.contains(&bundle_id)
}

/// Whether the application `pid`, named `name`, is a terminal emulator.
pub fn is_terminal(pid: Pid, name: &str, bundles: &BundleIds) -> bool {
    bundles.matches(pid, is_terminal_bundle) || is_terminal_name(name)
}

/// Returns the focused window of `app`.
///
/// Finder reports its desktop as its focused window when the user is working on the desktop. The
/// desktop is a scroll area, which capture never treats as a window.
pub fn focused_window(app: Pid) -> Option<Element> {
    let window = Element::application(app).element("AXFocusedWindow")?;
    let role = window.attribute("AXRole")?;
    ["AXWindow", "AXSheet", "AXDrawer"]
        .into_iter()
        .any(|window_role| is_text(&role, window_role))
        .then_some(window)
}

/// Calls `visit` with the owner and the layer of every on-screen window, front to back.
fn visit_windows(mut visit: impl FnMut(Pid, i32) -> ControlFlow<()>) {
    let options = ffi::CG_WINDOW_LIST_ON_SCREEN_ONLY | ffi::CG_WINDOW_LIST_EXCLUDE_DESKTOP;
    // SAFETY: the function returns a new array that the caller owns, or null.
    let list = unsafe { ffi::CGWindowListCopyWindowInfo(options, 0) };
    if list.is_null() {
        return;
    }
    // SAFETY: `list` is a non-null array that the caller owns.
    let windows: CFArray = unsafe { CFArray::wrap_under_create_rule(list) };
    // SAFETY: the static is a valid constant string for the lifetime of the process.
    let pid_key = unsafe { CFString::wrap_under_get_rule(ffi::kCGWindowOwnerPID) };
    // SAFETY: the static is a valid constant string for the lifetime of the process.
    let layer_key = unsafe { CFString::wrap_under_get_rule(ffi::kCGWindowLayer) };
    let number = |window: &CFDictionary, key: &CFString| {
        let value = window.find(key.as_CFTypeRef().cast())?;
        // SAFETY: the dictionary holds live CoreFoundation objects.
        unsafe { CFType::wrap_under_get_rule(*value) }.downcast::<CFNumber>()?.to_i32()
    };
    for window in &windows {
        // SAFETY: every entry of the window list is a dictionary.
        let window: CFDictionary = unsafe { CFDictionary::wrap_under_get_rule((*window).cast()) };
        if let Some(layer) = number(&window, &layer_key)
            && let Some(pid) = number(&window, &pid_key)
            && visit(Pid::new(pid), layer).is_break()
        {
            return;
        }
    }
}

/// Returns whether the login session of this process shows its lock screen.
pub fn screen_now() -> Screen {
    if screen_is_locked() {
        Screen::Locked
    } else {
        Screen::Unlocked
    }
}

/// Whether the login session of this process shows its lock screen.
fn screen_is_locked() -> bool {
    // SAFETY: the function returns a new dictionary that the caller owns, or null when the
    // process doesn't have a login session.
    let session = unsafe { ffi::CGSessionCopyCurrentDictionary() };
    if session.is_null() {
        return false;
    }
    // SAFETY: `session` is a non-null dictionary that the caller owns.
    let session: CFDictionary = unsafe { CFDictionary::wrap_under_create_rule(session) };
    let key = CFString::from_static_string("CGSSessionScreenIsLocked");
    session
        .find(key.as_CFTypeRef().cast())
        // SAFETY: the dictionary holds live CoreFoundation objects.
        .map(|value| unsafe { CFType::wrap_under_get_rule(*value) })
        .and_then(|value| value.downcast::<CFBoolean>())
        .is_some_and(bool::from)
}

pub fn set_messaging_timeout() {
    // SAFETY: the system-wide element is live. Setting the timeout on it sets the default for
    // every element of this process.
    unsafe {
        ffi::AXUIElementSetMessagingTimeout(
            Element::system_wide().as_ptr(),
            MESSAGING_TIMEOUT_SECS,
        );
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Terminal comes with every macOS, so the installed terminals always have a name.
    #[rstest]
    fn the_installed_terminals_have_localized_names() {
        let names = localized_terminal_names();
        assert!(!names.is_empty());
        let is_bundle_name = |name: &str| {
            std::path::Path::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
        };
        assert!(names.iter().all(|name| !name.is_empty() && !is_bundle_name(name)), "{names:?}");
    }

    #[rstest]
    fn no_running_application_has_an_empty_name() {
        assert_eq!(running_app_named(""), None);
    }
}
