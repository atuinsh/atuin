//! Threads an agent started that atuin has captured no message of yet, kept so that who each will
//! be shared with can be told before its first message.
//!
//! What it tells is what the config files covering the thread's directory say now. Nothing is
//! queued for it: its first message decides for good, as [`super::starts`] and the auto-sharer
//! decide for any thread.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use atuin_client::ai_session::HarnessSession;
use atuin_client::lua_config::Thread;
use atuin_octavo::pb;
use atuin_octavo::queue::{DirShares, OrgShare, ShareState};
use atuin_octavo::shares::Configs;
use parking_lot::Mutex;

use crate::octavo::QueuedShares;

/// How many new threads are kept. A thread is let go once this many have started after it, by
/// when its first message has long been captured, or it never got one.
const KEPT: usize = 64;

/// A thread an agent started in `dir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NewThread {
    pub(super) session: HarnessSession,
    pub(super) dir: PathBuf,
}

impl NewThread {
    /// Who the thread will be shared with once its first message is captured, as the config files
    /// covering its directory say: their organizations, opening, if it goes up at all.
    pub(super) async fn shares(self, configs: &Configs) -> QueuedShares {
        // Octavo stores no thread of a harness or session id it refuses, so none of it is shared.
        if pb::HarnessSession::of_thread(self.session.clone()).is_none() {
            return QueuedShares::default();
        }
        let uploads = configs.uploads_thread(Thread::new(&self.session, Some(self.dir.clone())));
        if !uploads.await.unwrap_or_else(|err| {
            tracing::warn!(
                ?err,
                "a new thread stays here, as a config file covering it can't be run"
            );
            false
        }) {
            return QueuedShares::default();
        }
        let orgs = configs.orgs_of_thread(vec![self.dir.clone()]).await.unwrap_or_else(|err| {
            tracing::warn!(?err, "a new thread's organizations can't be told");
            Vec::new()
        });
        QueuedShares {
            orgs: orgs
                .iter()
                .map(|org| OrgShare {
                    org: org.clone(),
                    state: ShareState::Opening,
                })
                .collect(),
            dirs: vec![DirShares {
                dir: self.dir,
                orgs,
            }],
            is_private: false,
            revoking_links: false,
        }
    }
}

/// The latest threads agents started, the newest first.
#[derive(Clone, Debug, Default)]
pub(super) struct NewThreads {
    threads: Arc<Mutex<VecDeque<NewThread>>>,
}

impl NewThreads {
    /// Keep `thread`, in place of an earlier start of the same session.
    pub(super) fn add(&self, thread: NewThread) {
        let mut threads = self.threads.lock();
        threads.retain(|kept| kept.session != thread.session);
        threads.push_front(thread);
        threads.truncate(KEPT);
    }

    /// The newest thread whose session id, as its agent names it, is `id`.
    pub(super) fn find(&self, id: &str) -> Option<NewThread> {
        let threads = self.threads.lock();
        threads.iter().find(|thread| AsRef::<str>::as_ref(&thread.session.session) == id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::ai_session::{HarnessKind, NativeSessionId};
    use rstest::rstest;

    use super::*;

    fn thread(harness: HarnessKind, id: &str, dir: &str) -> NewThread {
        NewThread {
            session: HarnessSession {
                harness,
                session: NativeSessionId::from(id.to_owned()),
            },
            dir: PathBuf::from(dir),
        }
    }

    #[rstest]
    #[case::unknown("other", None)]
    #[case::started("a", Some("/repo"))]
    fn finds_a_thread_by_its_agents_id(#[case] id: &str, #[case] dir: Option<&str>) {
        let threads = NewThreads::default();
        threads.add(thread(HarnessKind::ClaudeCode, "a", "/repo"));
        assert_eq!(threads.find(id).map(|thread| thread.dir), dir.map(PathBuf::from));
    }

    /// A session started again, as `/clear` and a resume start one, is where it started last.
    #[rstest]
    fn a_restart_replaces_the_earlier_start() {
        let threads = NewThreads::default();
        threads.add(thread(HarnessKind::ClaudeCode, "a", "/repo"));
        threads.add(thread(HarnessKind::ClaudeCode, "a", "/repo/sub"));
        assert_eq!(threads.find("a").map(|thread| thread.dir), Some(PathBuf::from("/repo/sub")));
        assert_eq!(threads.threads.lock().len(), 1);
    }

    #[rstest]
    fn the_oldest_go_past_the_most_kept() {
        let threads = NewThreads::default();
        for n in 0..=KEPT {
            threads.add(thread(HarnessKind::ClaudeCode, &n.to_string(), "/repo"));
        }
        assert_eq!(threads.find("0"), None);
        assert!(threads.find(&KEPT.to_string()).is_some());
    }

    /// Two agents' sessions of the same id are told apart by which started last.
    #[rstest]
    fn the_newest_of_two_agents_is_found() {
        let threads = NewThreads::default();
        threads.add(thread(HarnessKind::ClaudeCode, "a", "/claude"));
        threads.add(thread(HarnessKind::Codex, "a", "/codex"));
        assert_eq!(threads.find("a").map(|thread| thread.dir), Some(PathBuf::from("/codex")));
    }
}
