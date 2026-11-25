//! The watch on macOS, where the user at the screen owns the console.

use futures::future::BoxFuture;

use super::{Desktop, POLL, SessionWatch};

/// Sees this user at the screen, after a login or a switch back from another user.
///
/// On macOS, a helper finds a desktop exactly when its user is at the screen, so the watch knows
/// whether a helper would find one, and a mark changes nothing.
#[derive(Debug)]
pub struct Console<P> {
    /// Tells whether this user is at the screen.
    at_screen: P,
}

impl<P: Fn() -> bool> Console<P> {
    pub const fn new(at_screen: P) -> Self {
        Self { at_screen }
    }
}

impl<P: Fn() -> bool + Send + Sync> SessionWatch for Console<P> {
    fn desktop(&self) -> Desktop {
        if (self.at_screen)() {
            Desktop::Present
        } else {
            Desktop::Absent
        }
    }

    fn mark(&mut self) {}

    /// Resolves once this user is at the screen, at once when the user is there already.
    fn started(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            while !(self.at_screen)() {
                tokio::time::sleep(POLL).await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::time::Duration;

    use parking_lot::Mutex;
    use rstest::rstest;

    use super::*;

    /// Answers each look at the screen with the next answer, and repeats the last one.
    fn screen(answers: &[bool]) -> impl Fn() -> bool + Send + Sync + use<> {
        let answers = Arc::new(Mutex::new(answers.iter().copied().collect::<VecDeque<_>>()));
        move || {
            let mut answers = answers.lock();
            if answers.len() > 1 {
                answers.pop_front().expect("an answer")
            } else {
                *answers.front().expect("an answer")
            }
        }
    }

    #[rstest]
    #[case::at_the_screen(true, Desktop::Present)]
    #[case::away(false, Desktop::Absent)]
    fn knows_whether_a_helper_finds_a_desktop(#[case] at_screen: bool, #[case] desktop: Desktop) {
        let watch = Console::new(move || at_screen);

        assert_eq!(watch.desktop(), desktop);
    }

    /// Each answer is one poll's.
    #[rstest]
    #[case::away_then_back(&[false, true], true)]
    #[case::away_for_a_while_then_back(&[false, false, false, true], true)]
    #[case::at_the_screen(&[true], true)]
    #[case::away_throughout(&[false], false)]
    #[tokio::test(start_paused = true)]
    async fn sees_the_user_at_the_screen(#[case] answers: &[bool], #[case] started: bool) {
        let mut watch = Console::new(screen(answers));

        let seen = tokio::time::timeout(Duration::from_mins(1), watch.started()).await.is_ok();

        assert_eq!(seen, started);
    }
}
