//! The Accessibility permission: whether macOS trusts this process to use the AX API, how long
//! the trust lasts, and the alert that asks for it.

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use objc2_app_kit::NSRunningApplication;
use objc2_foundation::NSString;

use super::element::{Element, Pid};
use super::reader::Reader;
use super::{ffi, quirks};
use crate::readiness::AccessLifetime;
use crate::{Limits, render};

/// Whether to show the system's dialog that asks the user for the permission.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Prompt {
    Show,
    Skip,
}

pub fn is_trusted(prompt: Prompt) -> bool {
    // SAFETY: the static is a valid constant string for the lifetime of the process.
    let key = unsafe { CFString::wrap_under_get_rule(ffi::kAXTrustedCheckOptionPrompt) };
    let show = if prompt == Prompt::Show {
        CFBoolean::true_value()
    } else {
        CFBoolean::false_value()
    };
    let options = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), show.as_CFType())]);
    // SAFETY: `options` is a live dictionary.
    unsafe { ffi::AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) != 0 }
}

/// The settings pane that lists the programs with the Accessibility permission.
const SETTINGS_PANE: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

/// Opens the settings pane in which the user grants the permission.
pub fn open_settings() {
    let _ = std::process::Command::new("open").arg(SETTINGS_PANE).status();
}

/// Reads how long the permission lasts from the code signature of the running executable,
/// which macOS matches a granted permission against.
pub fn access_lifetime() -> AccessLifetime {
    match designated_requirement() {
        // An ad-hoc signature identifies the program by the hash of this build.
        Some(requirement) if requirement.contains("cdhash") => AccessLifetime::ThisBuild,
        Some(_) => AccessLifetime::Durable,
        None => AccessLifetime::Unknown,
    }
}

/// Reads the designated requirement of the running executable's code signature.
fn designated_requirement() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let output = std::process::Command::new("codesign").arg("-d").arg("-r-").arg(exe).output();
    let output = output.ok()?;
    // The tool prints the requirement to either stream, depending on its version.
    [output.stdout, output.stderr].iter().find_map(|stream| {
        let text = String::from_utf8_lossy(stream);
        let requirement = text.lines().find_map(|line| line.split_once("designated => "))?.1;
        Some(requirement.to_owned())
    })
}

/// Closes the alert that asks whether `program` may control the computer, once `program` has the
/// permission.
///
/// Leaves the agent of the alert alone when the agent shows an alert about another program, and
/// when the agent's windows can't be read.
///
/// - **Differs:** the alert stays open after the user grants the permission in System
///   Settings. Setup opens System Settings at the same moment that the alert appears, so the
///   alert waits behind System Settings, and the user finds it later.
/// - **Observed:** on macOS 13.7.4, the alert stayed open after the grant. The agent quits
///   when it is asked to, as other applications do.
pub fn close_permission_alert(program: &str) {
    let id = NSString::from_str(quirks::PERMISSION_ALERT);
    let running = NSRunningApplication::runningApplicationsWithBundleIdentifier(&id);
    for index in 0..running.count() {
        let app = running.objectAtIndex(index);
        let mut windows =
            Element::application(Pid::new(app.processIdentifier())).elements("AXWindows");
        let about_program = |window: Element| {
            Reader::read_window(&window, Limits::default())
                .is_some_and(|read| render(&read.root, usize::MAX).text.contains(program))
        };
        // An empty list can also mean that the AX call failed.
        if windows.entries() > 0 && windows.all(about_program) {
            let quit = app.terminate();
            tracing::debug!(quit, "asked the permission alert to quit");
        }
    }
}
