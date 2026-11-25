//! Finds out whether a window belongs to a browser's private browsing mode.
//!
//! A window counts as private only when it carries a mark of a private window. A check that
//! can't finish fails with an error, so that the read fails and capture doesn't store any of
//! the window.

use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use atuin_common::collections::ForgetfulMap;
use futures::future;
use futures::stream::{self, StreamExt as _, TryStreamExt as _};
use parking_lot::Mutex;
use tokio::time::Instant;
use tracing::debug;

use super::bus::{App, Bus, IN_FLIGHT, Object, Toolkit};
use super::roles::AtspiRole;
use super::{procs, quirks};
use crate::backend::{Error, Role};
use crate::private_windows::{
    self, FIREFOX_MARKER_ID, Marker, PrivateTitles, Translations, chromium_titles,
};

/// How many applications, windows, and documents the caches hold. A full cache is emptied,
/// and fills again as windows are read.
const MAX_ENTRIES: usize = 1_000;

/// How far above a document its window may be. The check of a deeper document fails.
const MAX_DEPTH: usize = 64;

/// How long a browser whose translations can't be read keeps its English titles before a read
/// tries the translations again.
const RETRY_TRANSLATIONS: Duration = Duration::from_secs(60);

/// The name of the directory beside a browser's executable that holds the browser's language
/// packs.
const PACKS: &str = "locales";
/// The pack whose formats are known, inside [`PACKS`].
const ENGLISH_PACK: &str = "en-US.pak";

/// How a browser marks its private windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateMarking {
    /// Firefox marks a private window with a button, and with its English title.
    Gecko,
    /// The browser translates the title of a private window in the packs beside its
    /// executable.
    Chromium,
}

impl PrivateMarking {
    /// How an application with `toolkit` marks its private windows. Returns `None` for an
    /// application that isn't a known browser.
    pub fn of(toolkit: &Toolkit) -> Option<Self> {
        if quirks::marks_private_windows(toolkit) {
            Some(Self::Gecko)
        } else if quirks::titles_private_windows_in_packs(toolkit) {
            Some(Self::Chromium)
        } else {
            None
        }
    }
}

/// What a read starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    Window,
    /// A document, such as a tab in the background, whose window is found above it.
    Document,
}

impl Subject {
    /// The subject of a read that starts from an element with `role`.
    ///
    /// An element of an unknown role is checked as a document. That check walks up to the
    /// element's window, which is the element itself when it is a window, so a background tab
    /// whose role didn't arrive is still checked against its window.
    pub fn of(role: Option<AtspiRole>) -> Self {
        match role.map(AtspiRole::role) {
            Some(Role::Document) | None => Self::Document,
            Some(_) => Self::Window,
        }
    }
}

/// A window and its title.
struct Titled {
    window: Object,
    title: String,
}

/// The titles of the private windows of one application.
struct KnownTitles {
    titles: Arc<PrivateTitles>,
    /// When the next read tries the browser's translations again, for titles that stand in
    /// for translations that couldn't be read.
    retry_at: Option<Instant>,
}

/// Remembers what each browser's private windows look like, which windows carry a marker,
/// and which window holds each document.
pub struct PrivateWindows {
    /// The titles of the private windows of each application.
    titles: Mutex<ForgetfulMap<App, KnownTitles>>,
    /// The windows that carry Firefox's marker.
    ///
    /// A window stays private for as long as it is open. A window without the marker is searched
    /// again on each read, because Firefox can expose the window before its toolbar.
    marked: Mutex<ForgetfulMap<Object, ()>>,
    /// The window of each document that was read on its own.
    windows: Mutex<ForgetfulMap<Object, Object>>,
    /// The titles of a browser whose translations can't be read.
    english: Arc<PrivateTitles>,
}

impl Default for PrivateWindows {
    fn default() -> Self {
        Self {
            titles: Mutex::new(ForgetfulMap::new(MAX_ENTRIES)),
            marked: Mutex::new(ForgetfulMap::new(MAX_ENTRIES)),
            windows: Mutex::new(ForgetfulMap::new(MAX_ENTRIES)),
            english: Arc::new(PrivateTitles::chromium_in_english()),
        }
    }
}

impl PrivateWindows {
    /// Whether `object`, which a read starts from, is part of a private window.
    ///
    /// `object` belongs to a browser, and `marking` says how the browser marks its private
    /// windows.
    ///
    /// # Errors
    ///
    /// Fails when the check can't tell, such as when the browser doesn't answer a call of the
    /// check in time.
    pub async fn is_private(
        &self,
        bus: &Bus,
        object: &Object,
        subject: Subject,
        marking: PrivateMarking,
    ) -> Result<bool, Error> {
        match subject {
            Subject::Window => self.is_private_window(bus, object, None, marking).await,
            Subject::Document => {
                let titled = self.window_of(bus, object).await?;
                self.is_private_window(bus, &titled.window, Some(titled.title), marking).await
            }
        }
    }

    /// Whether `window` is private. `title` is the window's title when the caller already read
    /// it.
    async fn is_private_window(
        &self,
        bus: &Bus,
        window: &Object,
        title: Option<String>,
        marking: PrivateMarking,
    ) -> Result<bool, Error> {
        let marker = async {
            match marking {
                PrivateMarking::Gecko => self.marker(bus, window).await,
                PrivateMarking::Chromium => Marker::Unused,
            }
        };
        let (marker, titles) = future::join(marker, self.titles(bus, window.app(), marking)).await;
        let titles = titles?;
        let title = match title {
            Some(title) => Some(title),
            None if marker == Marker::Present || titles.is_empty() => None,
            None => bus
                .name(window)
                .await
                .inspect_err(|error| debug!(%error, "the title of a browser window can't be read"))
                .ok(),
        };
        private_windows::is_private(marker, title.as_deref(), &titles)
    }

    /// The window that holds `document`, with the window's title. The window is the element
    /// right below the application's root.
    ///
    /// A tab that moves to another window keeps its cached window. That window is private
    /// exactly when the new window is, because browsers keep private tabs apart from normal
    /// ones. A cached window whose title can't be read has closed, so the document's window is
    /// looked up again.
    async fn window_of(&self, bus: &Bus, document: &Object) -> Result<Titled, Error> {
        let cached = self.windows.lock().get(document).cloned();
        if let Some(window) = cached {
            if let Ok(title) = bus.name(&window).await {
                return Ok(Titled { window, title });
            }
            self.windows.lock().remove(document);
        }
        let window = top_window(bus, document).await?;
        let title = bus.name(&window).await?;
        self.windows.lock().insert(document.clone(), window.clone());
        Ok(Titled { window, title })
    }

    /// Whether `window` carries Firefox's marker.
    async fn marker(&self, bus: &Bus, window: &Object) -> Marker {
        if self.marked.lock().contains_key(window) {
            return Marker::Present;
        }
        match has_marker(bus, window).await {
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

    /// The titles of the private windows of `app`.
    ///
    /// Titles from the browser's translations are found once for each application. When the
    /// translations can't be read, the English titles apply. When only some of them can be read,
    /// the titles that were read apply. In both cases, a read after [`RETRY_TRANSLATIONS`] tries
    /// the translations again.
    ///
    /// Fails when the process behind `app` can't be found.
    async fn titles(
        &self,
        bus: &Bus,
        app: &App,
        marking: PrivateMarking,
    ) -> Result<Arc<PrivateTitles>, Error> {
        let now = Instant::now();
        if let Some(known) = self.titles.lock().get(app)
            && known.retry_at.is_none_or(|retry_at| now < retry_at)
        {
            return Ok(Arc::clone(&known.titles));
        }
        let known = match marking {
            PrivateMarking::Gecko => KnownTitles {
                titles: Arc::new(quirks::gecko_private_titles()),
                retry_at: None,
            },
            PrivateMarking::Chromium => {
                let pid = bus.connection_pid(app).await.ok_or(Error::NoAnswer)?;
                let titles = tokio::task::spawn_blocking(move || titles_of_process(pid))
                    .await
                    .unwrap_or_default();
                match titles {
                    Some(Translations::Complete(titles)) => KnownTitles {
                        titles: Arc::new(titles),
                        retry_at: None,
                    },
                    Some(Translations::Partial(titles)) => {
                        debug!("some of the browser's translations can't be read");
                        KnownTitles {
                            titles: Arc::new(titles),
                            retry_at: Some(now + RETRY_TRANSLATIONS),
                        }
                    }
                    None => {
                        debug!(
                            "the browser's translations can't be read, so the English titles apply"
                        );
                        KnownTitles {
                            titles: Arc::clone(&self.english),
                            retry_at: Some(now + RETRY_TRANSLATIONS),
                        }
                    }
                }
            }
        };
        let titles = Arc::clone(&known.titles);
        self.titles.lock().insert(app.clone(), known);
        Ok(titles)
    }
}

/// Whether one of the toolbars of `window` holds Firefox's button that clears the private
/// session. The search leaves out the pages, which are large and never hold the button.
///
/// Fails when a child of the window doesn't tell its role, or when the search of a toolbar
/// fails before a toolbar with the button was found. Also fails for a main window without a
/// toolbar, because Firefox can expose a new window before its toolbars, and every main window
/// of Firefox has a toolbar once its tree is built.
async fn has_marker(bus: &Bus, window: &Object) -> Result<bool, Error> {
    let children = bus.children(window).await?;
    let mut toolbars = 0_usize;
    let searches = stream::iter(children)
        .map(|child| async move {
            let role = bus.role(&child).await?;
            Ok::<_, Error>((role == AtspiRole::TOOL_BAR).then_some(child))
        })
        .buffer_unordered(IN_FLIGHT)
        .try_filter_map(future::ok)
        .map_ok(|toolbar| async move {
            bus.has_attribute_below(&toolbar, "id", FIREFOX_MARKER_ID).await
        })
        .try_buffer_unordered(IN_FLIGHT);
    let mut searches = pin!(searches);
    while let Some(found) = searches.try_next().await? {
        toolbars += 1;
        if found {
            return Ok(true);
        }
    }
    // A dialog, such as a file chooser, doesn't have a toolbar.
    if toolbars == 0 && bus.role(window).await? == AtspiRole::FRAME {
        debug!("a main window of Firefox doesn't have a toolbar yet");
        return Err(Error::NoAnswer);
    }
    Ok(false)
}

/// The element right below the application's root that holds `object`.
async fn top_window(bus: &Bus, object: &Object) -> Result<Object, Error> {
    let mut current = object.clone();
    for _ in 0..MAX_DEPTH {
        let parent = bus.parent(&current).await?;
        if parent.is_above_windows() {
            return Ok(current);
        }
        current = parent;
    }
    Err(Error::Platform("the window of a document is too far above the document".into()))
}

/// The titles of private windows that the packs beside the executable of the process `pid`
/// translate.
///
/// The packs are looked for first through the process's own view of the file system. That view
/// differs from the helper's view when the browser runs in a sandbox. For example, a Flatpak
/// browser keeps its packs inside its sandbox.
fn titles_of_process(pid: u32) -> Option<Translations> {
    let packs = procs::beside_exe(pid, PACKS)?;
    titles_in_packs(packs.own_view).or_else(|| titles_in_packs(packs.host_view))
}

/// The titles of private windows that the packs in `dir` translate. Returns `None` when the
/// directory or its English pack can't be read.
fn titles_in_packs(mut dir: PathBuf) -> Option<Translations> {
    let packs = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "pak"))
        .filter(|path| !is_english(path));
    dir.push(ENGLISH_PACK);
    chromium_titles(&dir, packs)
}

fn is_english(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == ENGLISH_PACK)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;
    use crate::private_windows::pak_tests::{Built, Resource, build, dir, write};

    #[rstest]
    fn the_packs_of_a_directory_give_every_language(dir: TempDir) {
        let english = [Resource {
            id: 9,
            text: "$1 (Incognito)",
        }];
        let german = [Resource {
            id: 9,
            text: "$1 (Inkognito)",
        }];
        write(&dir, ENGLISH_PACK, &build(Built::Version5, &english, &[]));
        write(&dir, "de.pak", &build(Built::Version5, &german, &[]));
        write(&dir, "notes.txt", b"not a pack");
        let Some(Translations::Complete(titles)) = titles_in_packs(dir.path().to_owned()) else {
            panic!("the packs read");
        };
        assert!(titles.matches("Seite - Chromium (Inkognito)"));
        assert!(titles.matches("Page - Chromium (Incognito)"));
        assert!(!titles.matches("Page - Chromium"));
    }

    #[rstest]
    fn a_directory_without_the_english_pack_gives_none(dir: TempDir) {
        let german = [Resource {
            id: 9,
            text: "$1 (Inkognito)",
        }];
        write(&dir, "de.pak", &build(Built::Version5, &german, &[]));
        assert_eq!(titles_in_packs(dir.path().to_owned()), None);
        assert_eq!(titles_in_packs(dir.path().join("missing")), None);
    }

    #[rstest]
    #[case::english("/opt/chromium/locales/en-US.pak", true)]
    #[case::british("/opt/chromium/locales/en-GB.pak", false)]
    #[case::directory_named_like_the_pack("/opt/en-US.pak/de.pak", false)]
    fn the_english_pack_is_known_by_its_name(#[case] path: &str, #[case] expected: bool) {
        assert_eq!(is_english(Path::new(path)), expected);
    }

    /// The test binary doesn't have packs beside it.
    #[rstest]
    fn a_process_without_packs_gives_none() {
        assert_eq!(titles_of_process(std::process::id()), None);
    }
}
