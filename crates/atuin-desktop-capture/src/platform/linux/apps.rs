//! What each application on the bus is: the kind of application, and its toolkit.

use std::sync::Arc;
use std::time::Duration;

use atuin_common::collections::ForgetfulMap;
use futures::future;
use parking_lot::Mutex;
use tokio::time::Instant;

use super::bus::{App, Bus, CALL_TIMEOUT, Object, Toolkit, is_missing};
use super::pauses::{PausePolicy, Pauses};
use super::quirks;
use crate::backend::Error;

/// How many applications the cache holds. A full cache is emptied, and fills again as the
/// applications send events.
const MAX_PROFILES: usize = 1_000;

/// How long finding out about an application may take while an event waits. Every event
/// waits for it, and so do the events of other applications.
///
/// An application that just started can take a fifth of a second to answer while it draws its
/// first window.
pub const BUDGET: Duration = Duration::from_millis(250);

/// How long an application that exceeded the [`BUDGET`] counts as an ordinary application
/// for events, without calls.
const PAUSE: Duration = Duration::from_secs(2);

/// The pauses of [`Profiles`]. Each lookup that exceeds the budget pauses the application for
/// [`PAUSE`], because a read asks a paused application anyway, with the timeout of a call.
const PAUSES: PausePolicy = PausePolicy {
    slow: PAUSE,
    silent: PAUSE,
    max_slow: u32::MAX,
};

/// What waits for a profile, which sets how long finding the profile out may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waiter {
    /// An event waits, so the lookup gets the [`BUDGET`].
    ///
    /// An application that exceeds the budget is paused, and counts as an ordinary application
    /// for events until the pause ends.
    Event,
    /// A read or a listing of applications waits, so the lookup gets the timeout of a call.
    ///
    /// The lookup also asks a paused application, because the quirks of an application change
    /// what a read finds.
    Call,
}

impl Waiter {
    const fn budget(self) -> Duration {
        match self {
            Self::Event => BUDGET,
            Self::Call => CALL_TIMEOUT,
        }
    }
}

/// Whether an application on the bus shows windows of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppKind {
    Application,
    /// The application only draws the frames of the windows of other applications. See
    /// [`quirks::draws_frames`].
    FrameDrawer,
    /// The application is the desktop shell, whose own surfaces aren't the user's windows.
    /// See [`quirks::is_desktop_shell`].
    Shell,
}

/// What an application told about its toolkit.
#[derive(Debug, Clone)]
pub enum ToolkitLookup {
    Named(Arc<Toolkit>),
    /// The application answered that it doesn't name a toolkit.
    Unnamed,
    /// The application didn't answer, so its toolkit can be any toolkit.
    Failed,
}

impl ToolkitLookup {
    fn of(toolkit: Result<Toolkit, Error>) -> Self {
        match toolkit {
            Ok(toolkit) => Self::Named(Arc::new(toolkit)),
            Err(error) if is_missing(&error) => Self::Unnamed,
            Err(_) => Self::Failed,
        }
    }

    /// The toolkit, when the application named it.
    pub fn named(&self) -> Option<&Toolkit> {
        match self {
            Self::Named(toolkit) => Some(toolkit),
            Self::Unnamed | Self::Failed => None,
        }
    }
}

/// What the backend knows about an application.
#[derive(Debug, Clone)]
pub struct Profile {
    pub kind: AppKind,
    pub toolkit: ToolkitLookup,
}

impl Profile {
    /// The profile of an application that did not answer.
    pub const UNKNOWN: Self = Self {
        kind: AppKind::Application,
        toolkit: ToolkitLookup::Failed,
    };

    /// Asks `app` for its name and its toolkit, within `budget`.
    ///
    /// Fails with [`Error::NoAnswer`] when the application takes longer than that.
    async fn find(bus: &Bus, app: &App, budget: Duration) -> Result<Self, Error> {
        let root = Object::root(app.clone());
        let (name, toolkit) =
            tokio::time::timeout(budget, future::join(bus.name(&root), bus.toolkit(app)))
                .await
                .map_err(|_| Error::NoAnswer)?;
        let name = name?;
        let toolkit = ToolkitLookup::of(toolkit);
        let kind = if quirks::draws_frames(&name) {
            AppKind::FrameDrawer
        } else if quirks::is_desktop_shell(&name) {
            AppKind::Shell
        } else {
            AppKind::Application
        };
        Ok(Self { kind, toolkit })
    }

    /// Whether the application announces some of its dialogs only by adding them inside a
    /// window.
    ///
    /// The desktop shell adds its dialogs to its stage. See also
    /// [`quirks::shows_alerts_inside_windows`]. A process that draws window frames, such as
    /// mutter-x11-frames on GTK 4, doesn't have dialogs.
    pub fn adds_dialogs_inside_windows(&self) -> bool {
        match self.kind {
            AppKind::Shell => true,
            AppKind::FrameDrawer => false,
            AppKind::Application => {
                self.toolkit.named().is_some_and(quirks::shows_alerts_inside_windows)
            }
        }
    }
}

/// An application, with its profile.
pub struct KnownApp {
    /// The bus name that the profiles hold, which a caller can keep without allocating.
    pub app: App,
    pub profile: Profile,
}

/// The profile of each application that answered for its name and its toolkit.
pub struct Profiles(Mutex<Known>);

impl Default for Profiles {
    fn default() -> Self {
        Self(Mutex::new(Known {
            profiles: ForgetfulMap::new(MAX_PROFILES),
            paused: Pauses::new(PAUSES),
            first_relations: ForgetfulMap::new(MAX_PROFILES),
        }))
    }
}

struct Known {
    profiles: ForgetfulMap<App, Profile>,
    /// The applications that exceeded the [`BUDGET`], which go without calls for events.
    paused: Pauses,
    /// When the backend first asked each application for the relations of an element.
    first_relations: ForgetfulMap<App, Instant>,
}

impl Profiles {
    /// Finds out what `app` is, for `waiter`.
    ///
    /// An application that doesn't answer for its name or its toolkit counts as an ordinary
    /// application whose toolkit lookup failed. A later call asks the application again.
    pub async fn profile(&self, bus: &Bus, app: &App, waiter: Waiter) -> Profile {
        let known = match waiter {
            Waiter::Event => self.known(app.as_str()).map(|known| known.profile),
            Waiter::Call => self.found(app.as_str()),
        };
        if let Some(profile) = known {
            return profile;
        }
        let found = Profile::find(bus, app, waiter.budget()).await;
        self.note(app, found, waiter)
    }

    /// The application with the bus name `app`, with its profile, when the profile is known
    /// without calls, for an event. An application that is paused counts as an ordinary one.
    pub fn known(&self, app: &str) -> Option<KnownApp> {
        let known = self.0.lock();
        if let Some((app, profile)) = known.profiles.get_key_value(app) {
            return Some(KnownApp {
                app: app.clone(),
                profile: profile.clone(),
            });
        }
        let paused = known.paused.is_paused(app, Instant::now());
        drop(known);
        paused.then(|| KnownApp {
            app: App::new(app),
            profile: Profile::UNKNOWN,
        })
    }

    /// Notes that the backend is asking `app` for the relations of an element, which can make `app`
    /// start to build the trees of its pages. See [`quirks::PAGE_BUILD_TIME`].
    pub fn asks_for_relations(&self, app: &App) {
        self.0.lock().first_relations.get_or_insert_with(app.clone(), Instant::now);
    }

    /// Whether `app` may still be building the tree of a page, because the backend first asked
    /// `app` for relations less than [`quirks::PAGE_BUILD_TIME`] ago.
    pub fn may_be_building_pages(&self, app: &App) -> bool {
        self.0
            .lock()
            .first_relations
            .get(app)
            .is_some_and(|asked| asked.elapsed() < quirks::PAGE_BUILD_TIME)
    }

    /// The profile that the application with the bus name `app` reported.
    fn found(&self, app: &str) -> Option<Profile> {
        self.0.lock().profiles.get(app).cloned()
    }

    /// Keeps what a lookup of `app` for `waiter` found, and returns the profile to go on
    /// with.
    fn note(&self, app: &App, found: Result<Profile, Error>, waiter: Waiter) -> Profile {
        match found {
            Ok(profile) if matches!(profile.toolkit, ToolkitLookup::Failed) => {
                tracing::debug!(?app, "an application did not tell its toolkit");
                profile
            }
            Ok(profile) => {
                let mut known = self.0.lock();
                known.paused.answered(app.as_str());
                known.profiles.insert(app.clone(), profile.clone());
                profile
            }
            Err(Error::NoAnswer) if waiter == Waiter::Event => {
                tracing::debug!(?app, "an application took too long to tell what it is");
                self.0.lock().paused.answered_slowly(app, Instant::now());
                Profile::UNKNOWN
            }
            Err(error) => {
                tracing::debug!(?app, %error, "an application did not tell what it is");
                Profile::UNKNOWN
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::super::bus::ToolkitFamily;
    use super::*;

    const APP: &str = ":1.42";

    fn app() -> App {
        App::new(APP)
    }

    fn shell() -> Profile {
        Profile {
            kind: AppKind::Shell,
            toolkit: ToolkitLookup::Unnamed,
        }
    }

    #[rstest]
    #[case::gtk4_application(AppKind::Application, "4.18", true)]
    #[case::gtk3_application(AppKind::Application, "3.24", false)]
    #[case::shell(AppKind::Shell, "3.24", true)]
    #[case::gtk4_frame_drawer(AppKind::FrameDrawer, "4.18", false)]
    fn dialogs_inside_windows_come_from_shells_and_gtk4_applications(
        #[case] kind: AppKind,
        #[case] gtk: &str,
        #[case] expected: bool,
    ) {
        let profile = Profile {
            kind,
            toolkit: ToolkitLookup::Named(Arc::new(Toolkit {
                name: "GTK".to_owned(),
                family: ToolkitFamily::of("GTK", gtk),
            })),
        };
        assert_eq!(profile.adds_dialogs_inside_windows(), expected);
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn an_application_that_took_too_long_for_an_event_is_paused() {
        let profiles = Profiles::default();
        assert!(profiles.known(APP).is_none());
        let profile = profiles.note(&app(), Err(Error::NoAnswer), Waiter::Event);
        assert_eq!(profile.kind, AppKind::Application);
        assert_eq!(profiles.known(APP).map(|known| known.profile.kind), Some(AppKind::Application));
        assert!(profiles.found(APP).is_none(), "a read asks a paused application again");
        tokio::time::advance(PAUSE).await;
        assert!(profiles.known(APP).is_none(), "the pause is over");
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn an_application_that_took_too_long_for_a_read_is_not_paused() {
        let profiles = Profiles::default();
        let profile = profiles.note(&app(), Err(Error::NoAnswer), Waiter::Call);
        assert_eq!(profile.kind, AppKind::Application);
        assert!(profiles.known(APP).is_none());
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn an_application_that_failed_is_asked_again(
        #[values(Waiter::Event, Waiter::Call)] waiter: Waiter,
    ) {
        let profiles = Profiles::default();
        profiles.note(&app(), Err(Error::Platform("gone".into())), waiter);
        assert!(profiles.known(APP).is_none());
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn a_found_profile_is_kept_and_ends_the_pause(
        #[values(Waiter::Event, Waiter::Call)] waiter: Waiter,
    ) {
        let profiles = Profiles::default();
        profiles.note(&app(), Err(Error::NoAnswer), Waiter::Event);
        profiles.note(&app(), Ok(shell()), waiter);
        assert_eq!(profiles.known(APP).map(|known| known.profile.kind), Some(AppKind::Shell));
        assert_eq!(profiles.found(APP).map(|profile| profile.kind), Some(AppKind::Shell));
        tokio::time::advance(PAUSE).await;
        assert_eq!(profiles.known(APP).map(|known| known.profile.kind), Some(AppKind::Shell));
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn a_failed_toolkit_lookup_is_asked_again(
        #[values(Waiter::Event, Waiter::Call)] waiter: Waiter,
    ) {
        let profiles = Profiles::default();
        let failed = Profile {
            kind: AppKind::Application,
            toolkit: ToolkitLookup::Failed,
        };
        let profile = profiles.note(&app(), Ok(failed), waiter);
        assert!(matches!(profile.toolkit, ToolkitLookup::Failed));
        assert!(profiles.found(APP).is_none());
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn an_unknown_application_has_a_failed_toolkit_lookup(
        #[values(Waiter::Event, Waiter::Call)] waiter: Waiter,
    ) {
        let profiles = Profiles::default();
        let profile = profiles.note(&app(), Err(Error::NoAnswer), waiter);
        assert!(matches!(profile.toolkit, ToolkitLookup::Failed));
    }

    #[rstest]
    #[case::no_answer(Error::NoAnswer)]
    #[case::other(Error::Platform("the connection closed".into()))]
    fn a_toolkit_error_that_is_not_an_answer_fails_the_lookup(#[case] error: Error) {
        assert!(matches!(ToolkitLookup::of(Err(error)), ToolkitLookup::Failed));
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn pages_may_be_building_only_right_after_the_first_request() {
        let profiles = Profiles::default();
        assert!(!profiles.may_be_building_pages(&app()), "the application wasn't asked yet");
        profiles.asks_for_relations(&app());
        assert!(profiles.may_be_building_pages(&app()));
        tokio::time::advance(quirks::PAGE_BUILD_TIME / 2).await;
        profiles.asks_for_relations(&app());
        tokio::time::advance(quirks::PAGE_BUILD_TIME / 2).await;
        assert!(!profiles.may_be_building_pages(&app()), "a later request doesn't count");
    }
}
