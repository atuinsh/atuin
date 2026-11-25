//! What `atuin setup` does about desktop capture.
//!
//! Setup turns capture on and off, and offers to start capture that `atuin desktop-capture stop`
//! stopped. On macOS, setup also asks for the Accessibility permission.

mod path;
mod permission;
mod start;

use atuin_client::settings::{AppSelection, Settings, desktop_capture};
use atuin_daemon::{Signing, SigningFiles};
use eyre::Result;
use ratatui::DefaultTerminal;
use toml_edit::Value;

use self::permission::PermissionOutcome;
use self::start::StartOutcome;
use super::super::config::update_config_file;
use super::super::desktop_capture::{
    Autostart, CertificateOutcome, Conflict, HelperAccess, Logins, Notes, StopState,
    ensure_certificate, helper_access, start_through_daemon, stop_state,
};
use super::plan::{DesktopCapture, Plan};
use super::{Found, Scope, Status, Step, Updates, send, send_also, summary};

/// What setup does about desktop capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DesktopChange {
    TurnOn,
    /// Capture stays on, and setup does what turning capture on does, apart from an autostart entry
    /// that the desktop turned off, which stays off.
    ///
    /// Keeping capture on brings the files up to date after `atuin` moved, puts back a block that
    /// the user took out, and turns `toolkit-accessibility` on when the setting is off.
    KeepOn,
    TurnOff,
    /// Capture stays off, and `--no-desktop-capture` asks to remove what setup wrote for it.
    KeepOff,
}

impl DesktopChange {
    /// What `plan` does about desktop capture, as far as `scope` reaches.
    pub(super) fn of(plan: &Plan, found: &Found, scope: Scope<'_>) -> Option<Self> {
        if !scope.covers::<DesktopCapture>() {
            return None;
        }
        match (found.desktop_capture, plan.desktop_capture.0) {
            (false, true) => Some(Self::TurnOn),
            (true, true) => Some(Self::KeepOn),
            (true, false) => Some(Self::TurnOff),
            (false, false) => plan.asks.removes_desktop_capture.then_some(Self::KeepOff),
        }
    }

    const fn is_on(self) -> bool {
        matches!(self, Self::TurnOn | Self::KeepOn)
    }

    /// Whether the change turns capture on or off.
    const fn flips(self) -> bool {
        matches!(self, Self::TurnOn | Self::TurnOff)
    }
}

/// Whether a run of setup wrote the applications that desktop capture reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AppsWrite {
    Written,
    Kept,
}

/// The phrase of the summary that says what `change` and `apps` did to capture, when they did
/// anything.
///
/// `selection` holds the applications that capture reads after the run, and `stop` says whether
/// `atuin desktop-capture stop` keeps capture off.
fn capture_phrase(
    change: DesktopChange,
    apps: AppsWrite,
    selection: AppSelection<'_>,
    stop: StopState,
) -> Option<&'static str> {
    const ONCE_LISTED: &str = "capturing your desktop once you run `atuin desktop-capture apps`, \
                               which writes the applications that expose content into \
                               `desktop_capture.apps`";
    match (change, apps, selection, stop) {
        (DesktopChange::TurnOff, ..) => Some("not capturing your desktop"),
        (DesktopChange::KeepOff, ..)
        | (DesktopChange::KeepOn, AppsWrite::Kept, ..)
        | (DesktopChange::KeepOn, _, _, StopState::Stopped) => None,
        // A line of its own says that capture stays stopped, and how to start it.
        (DesktopChange::TurnOn, _, _, StopState::Stopped) => Some("turned desktop capture on"),
        (DesktopChange::TurnOn | DesktopChange::KeepOn, _, AppSelection::None, _) => {
            Some(ONCE_LISTED)
        }
        (DesktopChange::TurnOn, ..) => Some("capturing your desktop"),
        (DesktopChange::KeepOn, AppsWrite::Written, AppSelection::All, _) => {
            Some("capturing every application on your desktop")
        }
        (DesktopChange::KeepOn, AppsWrite::Written, AppSelection::Only(_), _) => {
            Some("capturing the applications in `desktop_capture.apps`")
        }
    }
}

/// The phrases of what a change to capture did, and of what failed, for the summary.
#[derive(Debug, Default)]
struct Phrases {
    done: Vec<String>,
    failed: Vec<String>,
}

/// Applies `change` to desktop capture.
///
/// Capture that ends up on gets the certificate that signs the helper on macOS, so that the
/// Accessibility permission lasts across updates. On Linux, it gets the files that start capture
/// at each login. It also gets Claude Code's prompt hook. Capture that ends up off loses all of
/// them. A running daemon reads the config's change by itself.
pub(super) async fn capture_desktop(
    change: DesktopChange,
    apps: AppsWrite,
    settings: &Settings,
    tx: &Updates,
) {
    // A run that leaves capture as it is says only what it changed.
    if change.flips() {
        send(tx, Step::DesktopCapture, Status::Running("Setting up desktop capture".into()));
    }
    let mut phrases = Phrases::default();
    let stop = if change.is_on() {
        stop_state(settings).await
    } else {
        StopState::NotStopped
    };
    let selection = settings.desktop_capture.selection();
    if let Some(phrase) = capture_phrase(change, apps, selection, stop) {
        phrases.done.push(phrase.to_owned());
    }
    let signing = Signing::for_platform();
    let logins = Logins::for_platform();
    let notes = if change.flips() {
        Notes::Always
    } else {
        Notes::WhenDone
    };
    if change.is_on() {
        if let Some(signing) = &signing {
            ensure_signing(signing, &mut phrases).await;
        }
        if let Some(logins) = &logins {
            let autostart = if change == DesktopChange::TurnOn {
                Autostart::TurnBackOn
            } else {
                Autostart::KeepDesktopsChoice
            };
            logins.start(autostart).await.add_to(&mut phrases.done, &mut phrases.failed, notes);
        }
        #[cfg(feature = "ai")]
        add_prompt_hook(&mut phrases).await;
    } else {
        if let Some(logins) = &logins {
            logins.stop().await.add_to(&mut phrases.done, &mut phrases.failed, notes);
        }
        #[cfg(feature = "ai")]
        remove_prompt_hook(&mut phrases).await;
        if let Some(signing) = &signing {
            remove_signing(signing, &mut phrases).await;
        }
    }
    if let Some(status) = summary(phrases.done, phrases.failed) {
        send(tx, Step::DesktopCapture, status);
    }
}

/// Creates the certificate that signs the helper, unless it exists.
async fn ensure_signing(signing: &Signing, phrases: &mut Phrases) {
    match ensure_certificate(signing).await {
        Ok(CertificateOutcome::Created) => phrases.done.push(
            "created a certificate so that the Accessibility permission lasts across updates"
                .to_owned(),
        ),
        Ok(CertificateOutcome::Present) => {}
        Err(e) => phrases.failed.push(format!(
            "couldn't create the certificate that keeps the Accessibility permission across \
             updates ({e})"
        )),
    }
}

/// Removes the certificate that signs the helper, with its keychain and the signed copy.
async fn remove_signing(signing: &Signing, phrases: &mut Phrases) {
    match signing.remove().await {
        Ok(SigningFiles::Removed) => phrases.done.push(
            "removed the certificate that signed desktop capture's helper, with its keychain"
                .to_owned(),
        ),
        Ok(SigningFiles::Absent) => {}
        Err(e) => phrases.failed.push(format!(
            "couldn't remove the certificate that signed desktop capture's helper ({e})"
        )),
    }
}

/// Registers Claude Code's prompt hook, when Atuin's Claude Code hooks are installed.
#[cfg(feature = "ai")]
async fn add_prompt_hook(phrases: &mut Phrases) {
    if let Err(e) = super::super::desktop_capture::ensure_prompt_hook().await {
        phrases.failed.push(format!("couldn't add the prompt hook to Claude Code ({e})"));
    }
}

/// Takes Claude Code's prompt hook out.
#[cfg(feature = "ai")]
async fn remove_prompt_hook(phrases: &mut Phrases) {
    use super::super::desktop_capture::PromptHookRemoval;

    match super::super::desktop_capture::remove_prompt_hook().await {
        Ok(PromptHookRemoval::Removed) => {
            phrases.done.push("took the prompt hook out of Claude Code".to_owned());
        }
        Ok(PromptHookRemoval::Absent) => {}
        Err(e) => {
            phrases.failed.push(format!("couldn't take the prompt hook out of Claude Code ({e})"));
        }
    }
}

/// The helper's access to the Accessibility permission, when this run sets up capture on a platform
/// that signs the helper.
///
/// A failed check of the access adds a failed line to the summary, and also gives `None`.
async fn permission_check(
    plan: &Plan,
    scope: Scope<'_>,
    settings: &Settings,
    signing: Option<&Signing>,
    tx: &Updates,
) -> Option<HelperAccess> {
    // Checking the access runs the helper, so setup checks only when the result can make setup
    // ask.
    if !sets_up_capture(plan, scope) {
        return None;
    }
    let signing = signing?;
    match helper_access(settings, Some(signing)).await {
        Ok(access) => Some(access),
        Err(e) => {
            let failed =
                format!("Couldn't check desktop capture's Accessibility permission ({e:#})");
            send_also(tx, Step::DesktopCapture, Status::Failed(failed));
            None
        }
    }
}

/// Whether this run sets up desktop capture, and capture ends up on.
fn sets_up_capture(plan: &Plan, scope: Scope<'_>) -> bool {
    scope.covers::<DesktopCapture>() && plan.desktop_capture.0
}

/// Says in the summary what desktop capture waits for, when this run sets up capture: a start after
/// `atuin desktop-capture stop`, and the Accessibility permission when the helper lacks it.
///
/// A run without the pages that ask calls this in place of [`offer_start`] and
/// [`ask_for_permission`].
pub(super) async fn report_what_capture_waits_for(
    plan: &Plan,
    scope: Scope<'_>,
    settings: &Settings,
    tx: &Updates,
) {
    if sets_up_capture(plan, scope) {
        report_stop(stop_state(settings).await, tx);
    }
    let signing = Signing::for_platform();
    let access = permission_check(plan, scope, settings, signing.as_ref(), tx).await;
    report_access(access, tx);
    if let Some(conflict) = later_conflict(plan).await {
        report_conflict(conflict, tx);
    }
}

/// Explains how another `atuin` will stop desktop capture, when capture ends up on and another
/// `atuin` will take the daemon.
async fn later_conflict(plan: &Plan) -> Option<String> {
    if !plan.desktop_capture.0 {
        return None;
    }
    Conflict::later().await.explain()
}

/// Says in the summary how another `atuin` will stop desktop capture.
fn report_conflict(conflict: String, tx: &Updates) {
    send(tx, Step::DesktopCaptureConflict, Status::Failed(conflict));
}

/// Shows the page that says that another `atuin` will stop desktop capture, and repeats the
/// explanation in the summary, when capture ends up on.
pub(super) async fn warn_about_conflict(
    terminal: &mut DefaultTerminal,
    plan: &Plan,
    tx: &Updates,
) -> Result<()> {
    let Some(conflict) = later_conflict(plan).await else {
        return Ok(());
    };
    terminal.clear()?;
    path::show(terminal, &conflict).await?;
    report_conflict(conflict, tx);
    Ok(())
}

/// Says in the summary that desktop capture waits for the Accessibility permission, when
/// `access` says that the permission is missing.
fn report_access(access: Option<HelperAccess>, tx: &Updates) {
    if access == Some(HelperAccess::Denied) {
        leave_permission_for_later(tx);
    }
}

/// Shows the page that offers to start desktop capture, when this run sets up capture and
/// `atuin desktop-capture stop` keeps capture off, and starts capture when the user asks.
pub(super) async fn offer_start(
    terminal: &mut DefaultTerminal,
    plan: &Plan,
    settings: &Settings,
    tx: &Updates,
) -> Result<()> {
    if !sets_up_capture(plan, Scope::All) || stop_state(settings).await != StopState::Stopped {
        return Ok(());
    }
    terminal.clear()?;
    match start::ask(terminal).await? {
        StartOutcome::Start => match start_through_daemon(settings).await {
            Ok(()) => {
                send_also(tx, Step::DesktopCapture, Status::Done("Started desktop capture".into()));
            }
            Err(e) => {
                let failed = format!(
                    "Couldn't start desktop capture ({e:#}): run `atuin desktop-capture start`"
                );
                send_also(tx, Step::DesktopCapture, Status::Failed(failed));
            }
        },
        StartOutcome::LeaveStopped => leave_stopped(tx),
    }
    Ok(())
}

/// The phrase that says that desktop capture stays stopped, and how to start it.
const STOPPED: &str = "Desktop capture stays stopped, as `atuin desktop-capture stop` asked: run \
                       `atuin desktop-capture start` to start it";

/// Says in the summary that desktop capture stays stopped, when `stop` says that a stop keeps
/// capture off.
fn report_stop(stop: StopState, tx: &Updates) {
    if stop == StopState::Stopped {
        leave_stopped(tx);
    }
}

/// Says in the summary that desktop capture stays stopped.
fn leave_stopped(tx: &Updates) {
    send_also(tx, Step::DesktopCapture, Status::Failed(STOPPED.into()));
}

/// Shows the permission page when desktop capture lacks the Accessibility permission, and
/// records how the user left the page. Turning capture off there turns it off in `plan`.
pub(super) async fn ask_for_permission(
    terminal: &mut DefaultTerminal,
    plan: &mut Plan,
    settings: &Settings,
    tx: &Updates,
) -> Result<()> {
    let signing = Signing::for_platform();
    let access = permission_check(plan, Scope::All, settings, signing.as_ref(), tx).await;
    if access != Some(HelperAccess::Denied) {
        return Ok(());
    }
    terminal.clear()?;
    match permission::ask(terminal, settings, signing.as_ref()).await {
        Ok(PermissionOutcome::Granted) => {
            // Setup wrote the applications to the config, so the config on disk says what
            // capture selects.
            let fresh = Settings::new();
            let config = fresh.as_ref().unwrap_or(settings);
            let selection = config.desktop_capture.selection();
            let after = start_after_grant(selection, stop_state(settings).await, settings).await;
            send_also(tx, Step::DesktopCapture, Status::Done(after.phrase().into()));
        }
        Ok(PermissionOutcome::TurnOff) => turn_off_desktop_capture(plan, settings, tx).await,
        Ok(PermissionOutcome::Later) => leave_permission_for_later(tx),
        Err(e) => {
            let failed = format!("Couldn't ask for the Accessibility permission ({e:#})");
            permission_failed(failed, tx);
        }
    }
    Ok(())
}

/// What desktop capture does once its helper has the Accessibility permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AfterGrant {
    /// Capture doesn't select any application, or a stop keeps capture off.
    Waits,
    /// The daemon started a helper.
    Starts,
    /// The daemon didn't answer, and will start a helper at its next try.
    StartsWithinAMinute,
}

impl AfterGrant {
    /// What the summary says.
    const fn phrase(self) -> &'static str {
        match self {
            Self::Waits => "Desktop capture has the Accessibility permission",
            Self::Starts => "Desktop capture has the Accessibility permission, and is starting",
            Self::StartsWithinAMinute => {
                "Desktop capture has the Accessibility permission, and starts within a minute"
            }
        }
    }
}

/// Asks the daemon to start a helper at once, when capture selects `selection` and `stop` says
/// that no stop keeps capture off.
///
/// The daemon tries a helper that lacked the permission again only a minute later, so capture
/// would otherwise start up to a minute after the grant.
async fn start_after_grant(
    selection: AppSelection<'_>,
    stop: StopState,
    settings: &Settings,
) -> AfterGrant {
    if matches!(selection, AppSelection::None) || stop == StopState::Stopped {
        return AfterGrant::Waits;
    }
    // What the daemon logs would land in the middle of the screen.
    let quiet = atuin_common::userio::log::SINK.push(atuin_common::userio::log::Nop);
    let started = start_through_daemon(settings).await;
    drop(quiet);
    match started {
        Ok(()) => AfterGrant::Starts,
        Err(e) => {
            tracing::debug!("could not start desktop capture after the grant: {e:#}");
            AfterGrant::StartsWithinAMinute
        }
    }
}

/// Turns desktop capture off after the user declined the Accessibility permission, and sets the
/// plan to match.
async fn turn_off_desktop_capture(plan: &mut Plan, settings: &Settings, tx: &Updates) {
    let off = vec![(desktop_capture::ENABLED_KEY, Value::from(false))];
    if let Err(e) = update_config_file(off).await {
        permission_failed(format!("Couldn't turn off desktop capture in your config ({e:#})"), tx);
        return;
    }
    plan.desktop_capture = DesktopCapture(false);
    capture_desktop(DesktopChange::TurnOff, AppsWrite::Kept, settings, tx).await;
}

/// The phrase that says how to grant the Accessibility permission later.
const PERMISSION_LATER: &str =
    "Desktop capture waits for the Accessibility permission: run `atuin setup` to grant it";

/// Says in the summary that desktop capture waits for the Accessibility permission.
fn leave_permission_for_later(tx: &Updates) {
    send_also(tx, Step::DesktopCapture, Status::Failed(PERMISSION_LATER.into()));
}

/// Says in the summary that the permission page ended with `failed`, and how to grant the
/// permission later.
fn permission_failed(failed: String, tx: &Updates) {
    send_also(tx, Step::DesktopCapture, Status::Failed(failed));
    leave_permission_for_later(tx);
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use tokio::sync::mpsc;

    use super::super::form::Form as _;
    use super::super::plan::Confirm;
    use super::super::tests::{found, setup};
    use super::super::{Update, view};
    use super::*;

    /// Capture that doesn't select any application doesn't start, and capture that a stop keeps off
    /// doesn't start either, so setup doesn't start a helper. A daemon that doesn't answer will
    /// start a helper at its next try.
    #[rstest]
    #[case::all(AppSelection::All, StopState::NotStopped, AfterGrant::StartsWithinAMinute)]
    #[case::only(
        AppSelection::Only(&["firefox".to_owned()]),
        StopState::NotStopped,
        AfterGrant::StartsWithinAMinute
    )]
    #[case::none(AppSelection::None, StopState::NotStopped, AfterGrant::Waits)]
    #[case::all_stopped(AppSelection::All, StopState::Stopped, AfterGrant::Waits)]
    #[case::only_stopped(
        AppSelection::Only(&["firefox".to_owned()]),
        StopState::Stopped,
        AfterGrant::Waits
    )]
    #[tokio::test]
    async fn a_grant_starts_capture_that_captures(
        #[case] selection: AppSelection<'_>,
        #[case] stop: StopState,
        #[case] expected: AfterGrant,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::utc();
        settings.daemon.socket_path = Some(dir.path().join("daemon.sock").try_into().unwrap());
        settings.daemon.pidfile_path = dir.path().join("atuin-daemon.pid").try_into().unwrap();
        settings.daemon.autostart = false;

        assert_eq!(start_after_grant(selection, stop, &settings).await, expected);
    }

    /// Capture that a stop keeps off doesn't capture, so the summary doesn't say that it does.
    #[rstest]
    fn a_stop_keeps_the_summary_from_saying_that_capture_captures(
        #[values(DesktopChange::TurnOn, DesktopChange::KeepOn)] change: DesktopChange,
        #[values(AppsWrite::Written, AppsWrite::Kept)] apps: AppsWrite,
        #[values(AppSelection::All, AppSelection::None)] selection: AppSelection<'_>,
    ) {
        let phrase = capture_phrase(change, apps, selection, StopState::Stopped);

        let expected = (change == DesktopChange::TurnOn).then_some("turned desktop capture on");
        assert_eq!(phrase, expected);
    }

    #[rstest]
    fn turning_capture_on_without_a_stop_says_that_it_captures() {
        let phrase = capture_phrase(
            DesktopChange::TurnOn,
            AppsWrite::Kept,
            AppSelection::All,
            StopState::NotStopped,
        );

        assert_eq!(phrase, Some("capturing your desktop"));
    }

    /// A run checks the permission only when the run sets up desktop capture, so that a run
    /// scoped to other parts says nothing about capture.
    #[rstest]
    #[case::full_setup(&[], true)]
    #[case::only_capture(&["--only", "--desktop-capture"], true)]
    #[case::only_elsewhere(&["--only", "--no-up-arrow"], false)]
    #[case::turned_off(&["--no-desktop-capture"], false)]
    fn the_permission_is_checked_only_while_capture_is_set_up(
        mut found: Found,
        #[case] args: &[&str],
        #[case] expected: bool,
    ) {
        found.daemon = true;
        found.desktop_capture = true;
        let cmd = setup(args);
        let plan = cmd.plan_at_screen(&found, Confirm::Yes).unwrap().effective();
        assert_eq!(sets_up_capture(&plan, cmd.scope()), expected);
    }

    /// Each change to capture or to its applications gets one phrase, and a run that changes
    /// neither gets none.
    #[rstest]
    #[case::turn_on_all(
        DesktopChange::TurnOn,
        AppsWrite::Written,
        AppSelection::All,
        Some("capturing your desktop")
    )]
    #[case::turn_on_none(
        DesktopChange::TurnOn,
        AppsWrite::Written,
        AppSelection::None,
        Some("capturing your desktop once you run")
    )]
    #[case::turn_on_kept_list(DesktopChange::TurnOn, AppsWrite::Kept, AppSelection::Only(&[]), Some("capturing your desktop"))]
    #[case::switch_to_all(
        DesktopChange::KeepOn,
        AppsWrite::Written,
        AppSelection::All,
        Some("capturing every application")
    )]
    #[case::switch_to_listed(
        DesktopChange::KeepOn,
        AppsWrite::Written,
        AppSelection::None,
        Some("capturing your desktop once you run")
    )]
    #[case::keep_none(DesktopChange::KeepOn, AppsWrite::Kept, AppSelection::None, None)]
    #[case::keep_all(DesktopChange::KeepOn, AppsWrite::Kept, AppSelection::All, None)]
    #[case::turn_off(
        DesktopChange::TurnOff,
        AppsWrite::Kept,
        AppSelection::All,
        Some("not capturing")
    )]
    #[case::keep_off(DesktopChange::KeepOff, AppsWrite::Kept, AppSelection::None, None)]
    fn the_summary_says_what_changed_about_capture(
        #[case] change: DesktopChange,
        #[case] apps: AppsWrite,
        #[case] selection: AppSelection<'_>,
        #[case] expected: Option<&str>,
    ) {
        let phrase = capture_phrase(change, apps, selection, StopState::NotStopped);
        match expected {
            Some(start) => {
                assert!(phrase.is_some_and(|phrase| phrase.starts_with(start)), "{phrase:?}");
            }
            None => assert_eq!(phrase, None),
        }
    }

    const CERTIFICATE: &str = "Capturing your desktop; created a certificate";

    /// What the `DesktopCapture` step says after `report` sends to a step that said
    /// [`CERTIFICATE`].
    fn desktop_steps_after(report: impl FnOnce(&Updates)) -> Vec<(Step, Status)> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut steps = vec![(Step::DesktopCapture, Status::Done(CERTIFICATE.into()))];
        report(&tx);
        while let Ok(update) = rx.try_recv() {
            match update {
                Update::Step(step, status) => view::set(&mut steps, step, status),
                Update::Also(step, status) => view::also(&mut steps, step, status),
                Update::Browser { .. } | Update::NeedKey(_) => {}
            }
        }
        steps
    }

    /// `--yes` never asks for the permission, so its summary says how to grant it, after what the
    /// step said already.
    #[rstest]
    #[case::denied(
        Some(HelperAccess::Denied),
        Status::Failed(
            "Capturing your desktop; created a certificate; desktop capture waits for the \
             Accessibility permission: run `atuin setup` to grant it"
                .into()
        )
    )]
    #[case::granted(Some(HelperAccess::Granted), Status::Done(CERTIFICATE.into()))]
    #[case::unchecked(None, Status::Done(CERTIFICATE.into()))]
    fn yes_says_how_to_grant_the_permission_later(
        #[case] access: Option<HelperAccess>,
        #[case] expected: Status,
    ) {
        let steps = desktop_steps_after(|tx| report_access(access, tx));
        assert_eq!(steps, [(Step::DesktopCapture, expected)]);
    }

    /// `--yes` never offers to start capture that `atuin desktop-capture stop` stopped, so its
    /// summary says how to start capture, after what the step said already.
    #[rstest]
    #[case::stopped(
        StopState::Stopped,
        Status::Failed(
            "Capturing your desktop; created a certificate; desktop capture stays stopped, as \
             `atuin desktop-capture stop` asked: run `atuin desktop-capture start` to start it"
                .into()
        )
    )]
    #[case::not_stopped(StopState::NotStopped, Status::Done(CERTIFICATE.into()))]
    fn yes_says_how_to_start_stopped_capture(#[case] stop: StopState, #[case] expected: Status) {
        let steps = desktop_steps_after(|tx| report_stop(stop, tx));
        assert_eq!(steps, [(Step::DesktopCapture, expected)]);
    }

    /// The explanation of another `atuin` that will stop desktop capture is several sentences long,
    /// so the summary shows it on a line of its own.
    #[rstest]
    fn the_conflict_has_a_line_of_its_own() {
        let conflict = "PATH runs /opt/homebrew/bin/atuin, another atuin. Run `brew unlink atuin`.";
        let steps = desktop_steps_after(|tx| report_conflict(conflict.into(), tx));
        assert_eq!(steps, [
            (Step::DesktopCapture, Status::Done(CERTIFICATE.into())),
            (Step::DesktopCaptureConflict, Status::Failed(conflict.into())),
        ]);
    }

    /// A page that ends in an error still says how to grant the permission later.
    #[rstest]
    fn a_failed_page_says_how_to_grant_the_permission_later() {
        let steps =
            desktop_steps_after(|tx| permission_failed("Couldn't ask (it broke)".into(), tx));
        let expected = "Capturing your desktop; created a certificate; couldn't ask (it broke); \
                        desktop capture waits for the Accessibility permission: run `atuin setup` \
                        to grant it";
        assert_eq!(steps, [(Step::DesktopCapture, Status::Failed(expected.into()))]);
    }
}
