//! What the supervisor knows of the helper's side of the link.

use std::time::Duration;

use atuin_desktop_capture::link::DaemonFrame;
use tokio::time::Instant;

use crate::desktop_capture::stop_file::Requested;

/// How long after a ping, or after the hello, the supervisor pings the helper again.
pub const PING_INTERVAL: Duration = Duration::from_secs(30);

/// How long a helper has to answer a ping before the supervisor gives up on the helper.
pub const PONG_TIMEOUT: Duration = Duration::from_secs(30);

/// Where the link to one helper stands. Each method updates the state and returns the frame to
/// send, if any, and the caller sends that frame.
#[derive(Debug)]
pub struct LinkState {
    progress: Progress,
    /// `None` until the helper is sent `Configure`.
    configured: Option<Told>,
    connected_at: Option<Instant>,
}

/// How far the helper has come in the link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Progress {
    AwaitingHello,
    /// The helper said hello, and has not reported a status yet.
    AwaitingStatus,
    Connected,
    NeedsPermission,
}

/// What the supervisor tracks of a helper that was sent `Configure`.
#[derive(Debug, Clone, Copy)]
struct Told {
    /// What the helper was last told about stopping.
    told: Requested,
    liveness: Liveness,
}

/// Where the supervisor stands in checking that the helper still answers.
#[derive(Debug, Clone, Copy)]
enum Liveness {
    /// The next ping goes out at `ping_at`.
    Idle {
        ping_at: Instant,
    },
    /// A ping went out at `sent`, and the helper has not answered it yet.
    Awaiting {
        sent: Instant,
    },
}

/// What to do when the time in [`LinkState::liveness_due`] comes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivenessCheck {
    /// Send a ping.
    Ping,
    /// The helper did not answer the last ping in time.
    Unanswered,
}

impl LinkState {
    pub const fn new() -> Self {
        Self {
            progress: Progress::AwaitingHello,
            configured: None,
            connected_at: None,
        }
    }

    pub fn awaits_hello(&self) -> bool {
        self.progress == Progress::AwaitingHello
    }

    /// Whether the helper reported how its start went: it connected, or it lacks the
    /// permission.
    pub fn has_outcome(&self) -> bool {
        matches!(self.progress, Progress::Connected | Progress::NeedsPermission)
    }

    pub fn needs_permission(&self) -> bool {
        self.progress == Progress::NeedsPermission
    }

    /// When the helper connected. `None` until it does.
    pub const fn connected_at(&self) -> Option<Instant> {
        self.connected_at
    }

    /// Notes that the helper said hello and was sent `Configure` in the mode of `requested`.
    pub fn configured(&mut self, requested: Requested) {
        self.progress = Progress::AwaitingStatus;
        self.configured = Some(Told {
            told: requested,
            liveness: Liveness::Idle {
                ping_at: Instant::now() + PING_INTERVAL,
            },
        });
    }

    pub fn connected(&mut self) {
        self.progress = Progress::Connected;
        self.connected_at = Some(Instant::now());
    }

    pub fn lacks_permission(&mut self) {
        self.progress = Progress::NeedsPermission;
    }

    /// What a configured helper was last told about stopping. `None` until the helper is
    /// configured.
    pub fn told(&self) -> Option<Requested> {
        self.configured.map(|configured| configured.told)
    }

    /// The `Pause` or `Resume` that tells a configured helper about `requested`, when the
    /// helper was told otherwise.
    pub fn tell<C>(&mut self, requested: Requested) -> Option<DaemonFrame<C>> {
        let configured = self.configured.as_mut()?;
        if configured.told == requested {
            return None;
        }
        configured.told = requested;
        Some(match requested {
            Requested::Running => DaemonFrame::Resume,
            Requested::Stopped => DaemonFrame::Pause,
        })
    }

    /// When the supervisor next pings the helper, or gives up on it. `None` until the helper is
    /// configured.
    pub fn liveness_due(&self) -> Option<Instant> {
        self.configured.map(|configured| match configured.liveness {
            Liveness::Idle { ping_at } => ping_at,
            Liveness::Awaiting { sent } => sent + PONG_TIMEOUT,
        })
    }

    /// Acts on the time in [`Self::liveness_due`]. After [`LivenessCheck::Ping`], the state
    /// awaits the answer.
    pub fn check_liveness(&mut self) -> LivenessCheck {
        let Some(configured) = &mut self.configured else {
            // `liveness_due` returns `None` for an unconfigured helper, so the supervisor doesn't
            // call this method for that helper. A ping is still harmless.
            return LivenessCheck::Ping;
        };
        if let Liveness::Awaiting { .. } = configured.liveness {
            return LivenessCheck::Unanswered;
        }
        configured.liveness = Liveness::Awaiting {
            sent: Instant::now(),
        };
        LivenessCheck::Ping
    }

    /// Takes the helper's answer to a ping. Returns `Err` when no ping awaits an answer.
    pub fn pong(&mut self) -> Result<(), ()> {
        let Some(configured) = &mut self.configured else {
            return Err(());
        };
        let Liveness::Awaiting { sent } = configured.liveness else {
            return Err(());
        };
        configured.liveness = Liveness::Idle {
            ping_at: sent + PING_INTERVAL,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// An event of the link, in the order that a test applies them.
    #[derive(Debug, Clone, Copy)]
    enum Event {
        Configured,
        Connected,
        LacksPermission,
    }

    /// What the queries of a [`LinkState`] answer.
    #[derive(Debug, Default, PartialEq, Eq)]
    struct Observed {
        awaits_hello: bool,
        configured: bool,
        needs_permission: bool,
        connected: bool,
    }

    #[rstest]
    #[case::new(&[], Observed {
        awaits_hello: true,
        ..Observed::default()
    })]
    #[case::configured(&[Event::Configured], Observed {
        configured: true,
        ..Observed::default()
    })]
    #[case::connected(&[Event::Configured, Event::Connected], Observed {
        configured: true,
        connected: true,
        ..Observed::default()
    })]
    #[case::refused_before_configure(&[Event::LacksPermission], Observed {
        needs_permission: true,
        ..Observed::default()
    })]
    #[case::refused(&[Event::Configured, Event::LacksPermission], Observed {
        configured: true,
        needs_permission: true,
        ..Observed::default()
    })]
    fn status_follows_the_events(#[case] events: &[Event], #[case] expected: Observed) {
        let mut link = LinkState::new();
        for event in events {
            match event {
                Event::Configured => link.configured(Requested::Running),
                Event::Connected => link.connected(),
                Event::LacksPermission => link.lacks_permission(),
            }
        }

        let observed = Observed {
            awaits_hello: link.awaits_hello(),
            configured: link.told().is_some(),
            needs_permission: link.needs_permission(),
            connected: link.connected_at().is_some(),
        };
        assert_eq!(observed, expected);
    }

    #[rstest]
    #[case::unconfigured(None, &[Requested::Stopped, Requested::Running], &[None, None])]
    #[case::already_running(Some(Requested::Running), &[Requested::Running], &[None])]
    #[case::configured_stopped(Some(Requested::Stopped), &[Requested::Stopped], &[None])]
    #[case::pause_once(Some(Requested::Running), &[Requested::Stopped, Requested::Stopped], &[
        Some(DaemonFrame::Pause),
        None,
    ])]
    #[case::pause_and_resume(Some(Requested::Running), &[
        Requested::Stopped,
        Requested::Running,
    ], &[Some(DaemonFrame::Pause), Some(DaemonFrame::Resume)])]
    #[case::resume_after_a_stopped_configure(Some(Requested::Stopped), &[Requested::Running], &[
        Some(DaemonFrame::Resume),
    ])]
    fn tell_sends_only_changes(
        #[case] configured: Option<Requested>,
        #[case] requests: &[Requested],
        #[case] expected: &[Option<DaemonFrame<()>>],
    ) {
        let mut link = LinkState::new();
        if let Some(configured) = configured {
            link.configured(configured);
        }

        let told = requests.iter().map(|&requested| link.tell(requested)).collect::<Vec<_>>();

        assert_eq!(told, expected);
    }

    /// A step of the liveness check, with what it returns.
    #[derive(Debug, Clone, Copy)]
    enum Step {
        Check(LivenessCheck),
        Pong(Result<(), ()>),
    }

    #[rstest]
    #[case::pings(&[Step::Check(LivenessCheck::Ping)])]
    #[case::gives_up(&[Step::Check(LivenessCheck::Ping), Step::Check(LivenessCheck::Unanswered)])]
    #[case::answered(&[
        Step::Check(LivenessCheck::Ping),
        Step::Pong(Ok(())),
        Step::Check(LivenessCheck::Ping),
    ])]
    #[case::unasked_pong(&[Step::Pong(Err(()))])]
    #[case::second_pong(&[Step::Check(LivenessCheck::Ping), Step::Pong(Ok(())), Step::Pong(Err(()))])]
    #[tokio::test(start_paused = true)]
    async fn liveness_pings_and_gives_up(#[case] steps: &[Step]) {
        let mut link = LinkState::new();
        assert_eq!(link.liveness_due(), None);
        link.configured(Requested::Running);
        assert_eq!(link.liveness_due(), Some(Instant::now() + PING_INTERVAL));

        for &step in steps {
            match step {
                Step::Check(expected) => {
                    tokio::time::advance(PING_INTERVAL).await;
                    assert_eq!(link.check_liveness(), expected, "{step:?}");
                }
                Step::Pong(expected) => assert_eq!(link.pong(), expected, "{step:?}"),
            }
        }
    }

    /// The next ping follows the last one by the interval, however late the answer came.
    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn the_next_ping_follows_the_last_one() {
        let mut link = LinkState::new();
        link.configured(Requested::Running);
        let sent = Instant::now();
        assert_eq!(link.check_liveness(), LivenessCheck::Ping);
        assert_eq!(link.liveness_due(), Some(sent + PONG_TIMEOUT));

        tokio::time::advance(Duration::from_secs(5)).await;
        link.pong().expect("a ping awaits an answer");

        assert_eq!(link.liveness_due(), Some(sent + PING_INTERVAL));
    }
}
