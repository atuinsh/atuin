//! Pauses the calls to an application that answered too slowly or not at all.
//!
//! Every call waits for its answer, and the events of other applications wait too, so an
//! application that is busy goes without calls for a while.

use std::collections::HashMap;
use std::time::Duration;

use tokio::time::Instant;

use super::bus::App;

/// How long an application stays paused after each kind of answer.
#[derive(Debug, Clone, Copy)]
pub struct PausePolicy {
    /// The pause after an answer that took too long.
    pub slow: Duration,
    /// The pause after a call that the application didn't answer, and after more slow answers
    /// in a row than [`max_slow`](Self::max_slow).
    pub silent: Duration,
    /// How many slow answers in a row get the [`slow`](Self::slow) pause.
    pub max_slow: u32,
}

/// An application that goes without calls for a while.
#[derive(Clone, Copy)]
struct Pause {
    until: Instant,
    /// How many answers in a row took too long.
    slow_answers: u32,
}

/// The applications that go without calls for a while, after they failed to answer or answered
/// too slowly.
pub struct Pauses {
    policy: PausePolicy,
    paused: HashMap<App, Pause>,
}

impl Pauses {
    pub fn new(policy: PausePolicy) -> Self {
        Self {
            policy,
            paused: HashMap::new(),
        }
    }

    /// Whether the application with the bus name `app` goes without calls at `now`.
    pub fn is_paused(&self, app: &str, now: Instant) -> bool {
        self.paused.get(app).is_some_and(|pause| now < pause.until)
    }

    /// Ends the pause of `app`, and its row of slow answers.
    pub fn answered(&mut self, app: &str) {
        self.paused.remove(app);
    }

    pub fn not_answered(&mut self, app: &App, now: Instant) {
        let silent = self.policy.silent;
        self.pause(app, now, |pause| Pause {
            until: now + silent,
            ..pause
        });
    }

    /// Pauses `app` after an answer that took too long. An application that is slow again right
    /// after its pause can count as one that doesn't answer.
    pub fn answered_slowly(&mut self, app: &App, now: Instant) {
        let policy = self.policy;
        self.pause(app, now, |pause| {
            let slow_answers = pause.slow_answers.saturating_add(1);
            let length = if slow_answers > policy.max_slow {
                policy.silent
            } else {
                policy.slow
            };
            Pause {
                until: now + length,
                slow_answers,
            }
        });
    }

    /// Changes the pause of `app` with `change`, and drops the pauses that ended a while ago.
    fn pause(&mut self, app: &App, now: Instant, change: impl FnOnce(Pause) -> Pause) {
        // A pause that ended within the last silence still counts the slow answers.
        let silent = self.policy.silent;
        self.paused.retain(|_, pause| now < pause.until + silent);
        let pause = self.paused.get(app).copied().unwrap_or(Pause {
            until: now,
            slow_answers: 0,
        });
        self.paused.insert(app.clone(), change(pause));
    }
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};

    use super::*;

    const SLOW: Duration = Duration::from_secs(1);
    const SILENT: Duration = Duration::from_secs(5);

    #[fixture]
    fn pauses() -> Pauses {
        Pauses::new(PausePolicy {
            slow: SLOW,
            silent: SILENT,
            max_slow: 1,
        })
    }

    const APP: &str = ":1.1";

    fn app() -> App {
        App::new(APP)
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn a_second_slow_answer_in_a_row_silences_the_application(mut pauses: Pauses) {
        pauses.answered_slowly(&app(), Instant::now());
        assert!(pauses.is_paused(APP, Instant::now()));
        tokio::time::advance(SLOW).await;
        assert!(!pauses.is_paused(APP, Instant::now()));
        pauses.answered_slowly(&app(), Instant::now());
        tokio::time::advance(SLOW).await;
        assert!(pauses.is_paused(APP, Instant::now()), "the second pause is a silence");
        tokio::time::advance(SILENT - SLOW).await;
        assert!(!pauses.is_paused(APP, Instant::now()));
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn an_answer_in_time_ends_the_row_of_slow_answers(mut pauses: Pauses) {
        pauses.answered_slowly(&app(), Instant::now());
        tokio::time::advance(SLOW).await;
        pauses.answered(APP);
        pauses.answered_slowly(&app(), Instant::now());
        tokio::time::advance(SLOW).await;
        assert!(!pauses.is_paused(APP, Instant::now()));
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn pauses_that_ended_long_ago_are_dropped(mut pauses: Pauses) {
        pauses.not_answered(&App::new(":1.1"), Instant::now());
        tokio::time::advance(SILENT * 2).await;
        pauses.answered_slowly(&App::new(":1.2"), Instant::now());
        assert_eq!(pauses.paused.len(), 1);
    }
}
