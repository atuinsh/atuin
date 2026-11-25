//! Finds out whether a window belongs to a browser's private browsing mode.
//!
//! A window counts as private only when it carries a mark of a private window. A check that
//! can't finish fails with an error, so that the read fails and capture doesn't store any of
//! the window.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use atuin_common::collections::ForgetfulMap;
use atuin_common::path::PathBufExt as _;
use core_foundation::base::CFType;
use objc2_app_kit::NSRunningApplication;
use parking_lot::Mutex;
use tracing::debug;

use super::desktop::BundleId;
use super::element::{
    AttributeNames, AttributeValues, Element, Elements, Pid, ax_error_of, elements_of, is_missing,
    string_of,
};
use super::quirks;
use crate::backend::Error;
use crate::private_windows::{
    self, FIREFOX_MARKER_ID, Marker, PrivateTitles, Translations, chromium_titles,
};

/// How many applications and windows the caches hold. A full cache is emptied, and fills again
/// as windows are read.
const MAX_ENTRIES: usize = 1_000;

/// How many levels below a window Firefox's "end private session" button sits: a group, the
/// toolbar, and the button.
const MARKER_DEPTH: usize = 3;

/// How far above an element its window may be. The check of a deeper element fails.
const MAX_WINDOW_DEPTH: usize = 64;

/// How long a browser whose translations can't be read keeps its English titles before a read
/// tries the translations again.
const RETRY_TRANSLATIONS: Duration = Duration::from_secs(60);

/// The frameworks of an application bundle, below its root.
const FRAMEWORKS: &str = "Contents/Frameworks";
/// The suffix of a language's directory in a framework's resources. For example, `en.lproj`.
const PACK_SUFFIX: &str = ".lproj";
const PACK_FILE: &str = "locale.pak";
/// The language of the pack that holds the known English formats.
const ENGLISH: &str = "en";

/// The attributes of an element that the marker search reads, in one call per element.
const MARKER_ATTRIBUTES: [&str; 3] = ["AXRole", "AXDOMIdentifier", "AXChildren"];
const ROLE: usize = 0;
const DOM_ID: usize = 1;
const CHILDREN: usize = 2;

/// How a browser marks its private windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrivateMarking {
    /// Firefox marks a private window with a button, and with its English title.
    Gecko,
    /// The browser marks a private window only in its title.
    Title,
}

/// What a browser's private windows look like.
#[derive(Debug, Clone)]
struct Marks {
    marking: PrivateMarking,
    titles: Arc<PrivateTitles>,
}

/// What capture knows about the private windows of one process.
struct Known {
    /// The bundle id, which tells a process apart from a later one with the same pid.
    bundle_id: Option<BundleId>,
    /// The marks of the process's private windows, or `None` for an application that isn't a
    /// known browser.
    marks: Option<Marks>,
    /// When the next read looks for the marks again, for marks that stand in for translations
    /// that couldn't be read.
    retry_at: Option<Instant>,
}

/// Remembers what each browser's private windows look like, and which windows carry a marker.
pub struct PrivateWindows {
    known: Mutex<ForgetfulMap<Pid, Known>>,
    /// The windows of Firefox that carry the marker.
    ///
    /// A window stays private for as long as it is open. A window without the marker is searched
    /// again on each read, because Firefox can expose the window before its toolbar.
    marked: Mutex<ForgetfulMap<Element, ()>>,
    marker_attributes: AttributeNames,
}

impl Default for PrivateWindows {
    fn default() -> Self {
        Self {
            known: Mutex::new(ForgetfulMap::new(MAX_ENTRIES)),
            marked: Mutex::new(ForgetfulMap::new(MAX_ENTRIES)),
            marker_attributes: AttributeNames::new(MARKER_ATTRIBUTES),
        }
    }
}

impl PrivateWindows {
    /// Whether `element`, which belongs to the process `pid` with the bundle id `bundle_id`,
    /// is part of a private window. A sheet or a dialog takes the privacy of its window.
    ///
    /// # Errors
    ///
    /// Fails when the check can't tell, such as when the browser doesn't answer an AX call of
    /// the check in time, or when the application of `pid` can't be looked up.
    pub fn is_private(
        &self,
        element: &Element,
        pid: Pid,
        bundle_id: Option<BundleId>,
    ) -> Result<bool, Error> {
        let Some(marks) = self.marks(pid, bundle_id)? else {
            return Ok(false);
        };
        let mut window = window_of(element)?;
        if marks.marking == PrivateMarking::Title
            && let Some(alert) = page_alert_in(&window)?
        {
            window = window_showing(&alert, &window, pid)?;
        }
        let marker = match marks.marking {
            PrivateMarking::Gecko => self.marker(&window),
            PrivateMarking::Title => Marker::Unused,
        };
        let title = if marker == Marker::Present || marks.titles.is_empty() {
            None
        } else {
            title_of(&window)
        };
        private_windows::is_private(marker, title.as_deref(), &marks.titles)
    }

    /// Whether `window` carries Firefox's marker.
    fn marker(&self, window: &Element) -> Marker {
        if self.marked.lock().contains_key(window) {
            return Marker::Present;
        }
        match has_marker(window, &self.marker_attributes) {
            Ok(true) => {
                self.marked.lock().insert(window.clone(), ());
                Marker::Present
            }
            Ok(false) => Marker::Absent,
            Err(error) => {
                debug!(%error, "the search for the private window's button failed");
                Marker::Unknown
            }
        }
    }

    /// The marks of the private windows of the process `pid`.
    ///
    /// The marks are found once for each process. When the browser's translations can't be read, or
    /// only in part, the titles that were read apply, and a read after [`RETRY_TRANSLATIONS`] tries
    /// the translations again.
    ///
    /// Fails when the application of `pid` can't be looked up. A failed lookup isn't kept.
    fn marks(&self, pid: Pid, bundle_id: Option<BundleId>) -> Result<Option<Marks>, Error> {
        let now = Instant::now();
        if let Some(known) = self.known.lock().get(&pid)
            && known.bundle_id == bundle_id
            && known.retry_at.is_none_or(|retry_at| now < retry_at)
        {
            return Ok(known.marks.clone());
        }
        let known = find_marks(pid, bundle_id, now)?;
        let marks = known.marks.clone();
        self.known.lock().insert(pid, known);
        Ok(marks)
    }
}

impl Known {
    /// What capture knows about an application that isn't a known browser.
    const fn unmarked(bundle_id: Option<BundleId>) -> Self {
        Self {
            bundle_id,
            marks: None,
            retry_at: None,
        }
    }

    fn gecko(bundle_id: Option<BundleId>) -> Self {
        Self {
            bundle_id,
            marks: Some(Marks {
                marking: PrivateMarking::Gecko,
                titles: Arc::new(quirks::gecko_private_titles()),
            }),
            retry_at: None,
        }
    }

    /// What capture knows about a browser that marks its private windows in their titles, as its
    /// `translations` give them.
    ///
    /// `english` gives the titles when the translations can't be read at all. Translations that
    /// weren't read in full are read again after [`RETRY_TRANSLATIONS`] from `now`.
    fn titled(
        bundle_id: Option<BundleId>,
        translations: Option<Translations>,
        english: fn() -> PrivateTitles,
        now: Instant,
    ) -> Self {
        let retry_at = match &translations {
            Some(Translations::Complete(_)) => None,
            Some(Translations::Partial(_)) => {
                debug!("some of the browser's translations can't be read");
                Some(now + RETRY_TRANSLATIONS)
            }
            None => {
                debug!("the browser's translations can't be read, so the English titles apply");
                Some(now + RETRY_TRANSLATIONS)
            }
        };
        let titles = match translations {
            Some(Translations::Complete(titles) | Translations::Partial(titles)) => titles,
            None => english(),
        };
        Self {
            bundle_id,
            marks: Some(Marks {
                marking: PrivateMarking::Title,
                titles: Arc::new(titles),
            }),
            retry_at,
        }
    }
}

/// Finds the marks of the private windows of the process `pid`, whose bundle id is
/// `bundle_id`, at `now`.
///
/// Fails when the process isn't known as a running application.
fn find_marks(pid: Pid, bundle_id: Option<BundleId>, now: Instant) -> Result<Known, Error> {
    let bundle = bundle_path(pid)?;
    // Safari is also known by its bundle, so that its check doesn't depend on the lookup of
    // its bundle id alone.
    let safari = bundle_id.as_ref().is_some_and(|bundle_id| quirks::is_safari(bundle_id.as_str()))
        || bundle.as_deref().is_some_and(quirks::is_safari_bundle);
    Ok(match bundle {
        _ if safari => Known::titled(
            bundle_id,
            quirks::safari_private_titles(),
            quirks::safari_english_titles,
            now,
        ),
        None => Known::unmarked(bundle_id),
        Some(bundle) if quirks::holds_gecko(&bundle) => Known::gecko(bundle_id),
        Some(bundle) => match titles_in_packs(bundle) {
            Packs::Absent => Known::unmarked(bundle_id),
            Packs::Read(Translations::Complete(titles) | Translations::Partial(titles))
                if titles.is_empty() =>
            {
                Known::unmarked(bundle_id)
            }
            Packs::Read(translations) => Known::titled(
                bundle_id,
                Some(translations),
                PrivateTitles::chromium_in_english,
                now,
            ),
            Packs::Unreadable => {
                Known::titled(bundle_id, None, PrivateTitles::chromium_in_english, now)
            }
        },
    })
}

/// The window that holds `element`, which is `element` itself for a window.
///
/// A sheet or a dialog group names its window. When the element that it names isn't a window,
/// the window is found among the element's ancestors.
///
/// Fails when an AX call fails, or when no window holds `element`.
fn window_of(element: &Element) -> Result<Element, Error> {
    if is_window(element)? {
        return Ok(element.clone());
    }
    let named =
        element.try_attribute("AXWindow").map_err(ax_failure)?.and_then(Element::from_value);
    if let Some(named) = named
        && is_window(&named)?
    {
        return Ok(named);
    }
    let mut current = element.clone();
    for _ in 0..MAX_WINDOW_DEPTH {
        current = current
            .try_attribute("AXParent")
            .map_err(ax_failure)?
            .and_then(Element::from_value)
            .ok_or(Error::NoAnswer)?;
        if is_window(&current)? {
            return Ok(current);
        }
    }
    debug!("the window of an element of a browser can't be found");
    Err(Error::NoAnswer)
}

/// The page's alert that `window` holds, when `window` is a window of its own for the alert of a
/// page: a window other than a standard window, whose only child is the alert.
///
/// See [`quirks::holds_page_alert`].
///
/// Fails when an AX call fails.
fn page_alert_in(window: &Element) -> Result<Option<Element>, Error> {
    if subrole_of(window)?.as_deref() == Some("AXStandardWindow") {
        return Ok(None);
    }
    let mut children = children_of(window)?;
    let (Some(child), None) = (children.next(), children.next()) else {
        return Ok(None);
    };
    let is_alert = subrole_of(&child)?.is_some_and(|subrole| quirks::is_page_alert(&subrole));
    Ok(is_alert.then_some(child))
}

/// The window of the process `pid` that shows `alert`, which `alert_window` holds.
///
/// A browser that is built on Chromium shows a page's alert in a window of its own, and that
/// window's attributes don't name the page's window. The page's window holds the same alert
/// element two levels down, so the alert takes the privacy of that window. The windows that
/// hold only an alert are skipped, so that the alert never takes the privacy of another
/// alert's window.
///
/// Fails when an AX call fails, or when no other window holds `alert`. Every read of an alert
/// that no page's window holds therefore fails, such as an alert of the browser itself in a
/// window of its own.
fn window_showing(alert: &Element, alert_window: &Element, pid: Pid) -> Result<Element, Error> {
    let windows = Element::application(pid).try_attribute("AXWindows").map_err(ax_failure)?;
    for window in windows.as_ref().map(elements_of).unwrap_or_default() {
        if window == *alert_window || page_alert_in(&window)?.is_some() {
            continue;
        }
        for child in children_of(&window)? {
            if children_of(&child)?.any(|grandchild| grandchild == *alert) {
                return Ok(window);
            }
        }
    }
    debug!("the window that shows a page's alert can't be found");
    Err(Error::NoAnswer)
}

/// The subrole of `element`, which is `None` for an element without a subrole.
///
/// Fails when the AX call fails.
fn subrole_of(element: &Element) -> Result<Option<String>, Error> {
    let subrole = element.try_attribute("AXSubrole").map_err(ax_failure)?;
    Ok(subrole.as_ref().and_then(string_of))
}

/// The children of `element`.
///
/// Fails when the AX call fails.
fn children_of(element: &Element) -> Result<Elements, Error> {
    let children = element.try_attribute("AXChildren").map_err(ax_failure)?;
    Ok(children.as_ref().map(elements_of).unwrap_or_default())
}

/// Whether `element` has the role of a window. A sheet or a drawer doesn't.
fn is_window(element: &Element) -> Result<bool, Error> {
    let role = element.try_attribute("AXRole").map_err(ax_failure)?;
    Ok(role.as_ref().and_then(string_of).as_deref() == Some("AXWindow"))
}

/// The title of `window`, which is empty for a window without a title. Returns `None` when the
/// title can't be read.
fn title_of(window: &Element) -> Option<String> {
    match window.try_attribute("AXTitle") {
        Ok(title) => Some(title.as_ref().and_then(string_of).unwrap_or_default()),
        Err(error) => {
            debug!(error, "the title of a browser window can't be read");
            None
        }
    }
}

/// Whether an element near the top of `window` is Firefox's button that clears the private
/// session. Web areas are skipped, because the button is part of the browser's own interface.
///
/// Fails when an AX call fails before the button was found. Also fails for a standard window
/// without a toolbar, because Firefox can expose a new window before its toolbars, and every
/// main window of Firefox has a toolbar once its tree is built.
fn has_marker(window: &Element, names: &AttributeNames) -> Result<bool, Error> {
    let mut level = window
        .try_attribute("AXChildren")
        .map_err(ax_failure)?
        .map_or_else(Vec::new, |children| elements_of(&children).collect());
    let mut next = Vec::new();
    let mut toolbars = 0_usize;
    for depth in 1..=MARKER_DEPTH {
        // The search order doesn't matter, and popping keeps the buffer.
        while let Some(element) = level.pop() {
            let values = element.attributes(names).ok_or(Error::NoAnswer)?;
            let role = value_at(&values, ROLE)?;
            let role = role.as_ref().and_then(string_of);
            match role.as_deref() {
                Some("AXButton") => {
                    let id = value_at(&values, DOM_ID)?;
                    if id.as_ref().and_then(string_of).as_deref() == Some(FIREFOX_MARKER_ID) {
                        return Ok(true);
                    }
                }
                Some("AXToolbar") => toolbars += 1,
                Some("AXWebArea") => continue,
                _ => {}
            }
            if depth < MARKER_DEPTH
                && let Some(children) = value_at(&values, CHILDREN)?
            {
                next.extend(elements_of(&children));
            }
        }
        std::mem::swap(&mut level, &mut next);
    }
    // A dialog doesn't have a toolbar.
    if toolbars == 0 && subrole_of(window)?.as_deref() == Some("AXStandardWindow") {
        debug!("a main window of Firefox doesn't have a toolbar yet");
        return Err(Error::NoAnswer);
    }
    Ok(false)
}

/// The value at `index` of `values`, or `None` when the element lacks the attribute.
///
/// Fails when the call couldn't read the attribute.
fn value_at(values: &AttributeValues, index: usize) -> Result<Option<CFType>, Error> {
    let value = values.get(index).ok_or(Error::NoAnswer)?;
    match ax_error_of(&value) {
        None => Ok(Some(value)),
        Some(error) if is_missing(error) => Ok(None),
        Some(error) => Err(ax_failure(error)),
    }
}

fn ax_failure(error: i32) -> Error {
    debug!(error, "an AX call of the private-window check failed");
    Error::NoAnswer
}

/// The root of the bundle of the process `pid`, or `None` for an application without a bundle.
///
/// Fails when the process isn't known as a running application, or when an application with a
/// bundle id doesn't tell where its bundle is.
fn bundle_path(pid: Pid) -> Result<Option<PathBuf>, Error> {
    let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid.raw()) else {
        debug!("the application of a window can't be looked up");
        return Err(Error::NoAnswer);
    };
    match app.bundleURL().and_then(|url| url.path()) {
        Some(path) => Ok(Some(PathBuf::from(path.to_string()))),
        None if app.bundleIdentifier().is_some() => {
            debug!("an application with a bundle id doesn't tell where its bundle is");
            Err(Error::NoAnswer)
        }
        None => Ok(None),
    }
}

/// What the frameworks of a bundle hold of a browser's translations.
#[derive(Debug, PartialEq, Eq)]
enum Packs {
    /// The bundle doesn't have a framework with an English pack, so the application isn't a
    /// browser that is built on Chromium.
    Absent,
    /// The titles of private windows that the packs translate. The titles are empty for an
    /// application that doesn't have private windows, such as an Electron application.
    Read(Translations),
    /// The frameworks, the list of languages, or the English pack can't be read.
    Unreadable,
}

/// What the packs in the frameworks of the bundle at `bundle` translate.
fn titles_in_packs(mut bundle: PathBuf) -> Packs {
    bundle.push(FRAMEWORKS);
    let frameworks = match std::fs::read_dir(&bundle) {
        Ok(frameworks) => frameworks,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Packs::Absent,
        Err(error) => {
            debug!(%error, "the frameworks of a bundle can't be listed");
            return Packs::Unreadable;
        }
    };
    let resources = frameworks.into_iter().find_map(|entry| {
        let mut resources = entry.ok()?.path();
        resources.push("Resources");
        english_pack(&resources).is_file().then_some(resources)
    });
    let Some(resources) = resources else {
        return Packs::Absent;
    };
    let languages = match std::fs::read_dir(&resources) {
        Ok(languages) => languages,
        Err(error) => {
            debug!(%error, "the languages of a browser's framework can't be listed");
            return Packs::Unreadable;
        }
    };
    let packs = languages.filter_map(|entry| {
        let entry = entry.ok()?;
        let name = entry.file_name();
        let language = name.to_str()?.strip_suffix(PACK_SUFFIX)?;
        (language != ENGLISH).then(|| entry.path().joined(PACK_FILE))
    });
    chromium_titles(&english_pack(&resources), packs).map_or(Packs::Unreadable, Packs::Read)
}

/// The English pack in the `Resources` directory of a framework.
fn english_pack(resources: &Path) -> PathBuf {
    let mut pack = resources.join(ENGLISH);
    pack.as_mut_os_string().push(PACK_SUFFIX);
    pack.push(PACK_FILE);
    pack
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;
    use crate::private_windows::pak_tests::{Built, Resource, build, dir};

    /// Writes `pack` as the pack of `language` in the framework `framework` of the bundle in
    /// `dir`.
    fn add_pack(dir: &TempDir, framework: &str, language: &str, pack: &[u8]) {
        let mut path = dir.path().join(FRAMEWORKS);
        path.push(framework);
        path.push("Resources");
        path.push(format!("{language}{PACK_SUFFIX}"));
        std::fs::create_dir_all(&path).unwrap();
        path.push(PACK_FILE);
        std::fs::write(path, pack).unwrap();
    }

    fn pack(text: &str) -> Vec<u8> {
        build(Built::Version5, &[Resource { id: 9, text }], &[])
    }

    #[rstest]
    fn the_packs_of_a_framework_give_every_language(dir: TempDir) {
        add_pack(&dir, "Helpers.framework", "de", &pack("x"));
        add_pack(&dir, "Chromium Framework.framework", ENGLISH, &pack("$1 (Incognito)"));
        add_pack(&dir, "Chromium Framework.framework", "de", &pack("$1 (Inkognito)"));
        let Packs::Read(Translations::Complete(titles)) = titles_in_packs(dir.path().to_owned())
        else {
            panic!("the packs read");
        };
        assert!(titles.matches("Seite - Chromium (Inkognito)"));
        assert!(titles.matches("Page - Chromium (Incognito)"));
    }

    #[rstest]
    fn a_bundle_without_frameworks_has_no_packs(dir: TempDir) {
        assert_eq!(titles_in_packs(dir.path().to_owned()), Packs::Absent);
        std::fs::create_dir_all(dir.path().join(FRAMEWORKS)).unwrap();
        assert_eq!(titles_in_packs(dir.path().to_owned()), Packs::Absent);
    }

    #[rstest]
    fn an_unreadable_english_pack_makes_the_packs_unreadable(dir: TempDir) {
        add_pack(&dir, "Chromium Framework.framework", ENGLISH, b"not a pack");
        assert_eq!(titles_in_packs(dir.path().to_owned()), Packs::Unreadable);
    }

    #[rstest]
    fn an_electron_application_has_empty_titles(dir: TempDir) {
        add_pack(&dir, "Electron Framework.framework", ENGLISH, &pack("Copy"));
        assert_eq!(
            titles_in_packs(dir.path().to_owned()),
            Packs::Read(Translations::Complete(PrivateTitles::default()))
        );
    }

    #[rstest]
    fn a_bundle_without_gecko_is_not_gecko(dir: TempDir) {
        assert!(!quirks::holds_gecko(dir.path()));
        let library = dir.path().join("Contents/MacOS");
        std::fs::create_dir_all(&library).unwrap();
        std::fs::write(library.join("XUL"), b"").unwrap();
        assert!(quirks::holds_gecko(dir.path()));
    }
}
