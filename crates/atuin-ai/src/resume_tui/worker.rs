//! Runs source queries off the UI thread.
//!
//! Two lanes, so a search never waits behind the selected session's details:
//! - searches, tagged with a generation. Queued ones are coalesced (only the newest matters while
//!   typing), and the UI drops answers for any generation but the newest, keeping the old list on
//!   screen until then;
//! - details (preview, children, plan) for the selected session. Only the newest request of each
//!   kind is kept: while the selection moves, the sessions it passed over are never loaded.
//!   Restoring a session from sync (or catching its copy here up with sync), or continuing it in
//!   another harness, which only an enter or tab asks for, goes first: only the newest accepted
//!   session's restore or catch-up is kept, and one dropped
//!   before it started is reported ([`Response::Abandoned`]) so that the picker asks again if the
//!   user comes back to it. Likewise only the newest continuation or fork is kept, and one is
//!   dropped when the picker stops waiting on it ([`Request::CancelContinue`]); once the picker is
//!   gone, nothing queued runs. One already running finishes and reports as usual: its files are
//!   left as written, not deleted, since capture may already have read and synced them. Then the
//!   plan an enter or tab is waiting on, which is never dropped for another session's (see
//!   [`Request::Accept`]).

use std::path::PathBuf;
use std::sync::Arc;

use atuin_client::ai_session::{HarnessKind, HarnessSession, SourceId};
use atuin_client::settings::AiSessionFilterMode as FilterMode;
use atuin_common::harnesstools::continuation::{self, Flattened};
use tokio::sync::mpsc;

use super::catchup::{self, CatchUp};
use super::resumer::{Continued, Forked, NotResumable, Restore, Resume, ResumePlan, Resumer};
use super::source::{SessionFilter, SessionPreview, SessionRow, SessionSource};

#[derive(Debug)]
pub enum Request {
    Search {
        generation: u64,
        mode: FilterMode,
        filter: SessionFilter,
    },
    Preview(HarnessSession),
    /// The forks grouped under a root.
    Children(HarnessSession),
    /// Plan resuming a session (may walk the harness's session directories).
    Plan(Box<SessionRow>),
    /// Plan resuming the session an enter, tab or ctrl-y is waiting on: a [`Request::Plan`] that
    /// a plan for another session (the selection's, as it settles) can't take the place of. Only
    /// a newer accept can, which the picker then waits on instead.
    Accept(Box<SessionRow>),
    /// Write out the transcript of a session planned with a restore, and plan resuming it.
    Restore(Box<SessionRow>, Restore),
    /// Catch the copy here of a session up with sync, and plan resuming it. Kept as a restore
    /// is, in its place.
    CatchUp(Box<SessionRow>),
    /// Count what continuing a session in another harness would flatten (reads all of it).
    Flatten(HarnessSession, PathBuf),
    /// Write a session out as a new session of another harness, and plan resuming that. The id
    /// comes back with the answer. Only the newest is kept: one a newer one replaces before it
    /// started never runs (its answer would be ignored, as the picker waits on the newer id).
    Continue(Box<SessionRow>, HarnessKind, u64),
    /// Write a session out as a fork of it (from a head, when one is given), and plan resuming
    /// that. Answered as a continuation is, and in its place: a newer pick supersedes either.
    Fork(Box<SessionRow>, u64, Option<SourceId>),
    /// The picker no longer waits on a continuation or fork (the user chose something else):
    /// drop the one not yet started, so nothing is written for it.
    CancelContinue,
}

#[derive(Debug)]
pub enum Response {
    Results {
        generation: u64,
        mode: FilterMode,
        rows: Result<Vec<SessionRow>, String>,
    },
    Preview(HarnessSession, SessionPreview),
    Children(HarnessSession, Vec<SessionRow>),
    Plan(HarnessSession, Result<Resume, NotResumable>),
    /// The session is restored (or couldn't be): the plan that resumes it.
    Restored(HarnessSession, Result<ResumePlan, NotResumable>),
    /// The copy here is caught up (or catching up needs a choice, or failed).
    CaughtUp(HarnessSession, Result<CatchUp, NotResumable>),
    /// The session's restore or catch-up was dropped before it started, for a newer accepted
    /// session's: it has to be asked for again.
    Abandoned(HarnessSession),
    /// What continuing the session in another harness would flatten (or why that can't be read).
    Flattened(HarnessSession, Result<Flattened, String>),
    /// The continuation with this id is written (or couldn't be).
    Continued(u64, Result<Continued, NotResumable>),
    /// The fork with this id is written (or couldn't be).
    Forked(u64, Result<Forked, NotResumable>),
}

/// Sends requests to the worker's lanes. The worker stops when this is dropped.
#[derive(Clone)]
pub struct Requests {
    searches: mpsc::UnboundedSender<Request>,
    details: mpsc::UnboundedSender<Request>,
}

impl Requests {
    /// Queue `request`, or drop it if the worker has stopped (the response channel then closes).
    pub fn send(&self, request: Request) {
        let lane = match request {
            Request::Search { .. } => &self.searches,
            _ => &self.details,
        };
        let _ = lane.send(request);
    }
}

/// Start the worker. It stops when the [`Requests`] are dropped.
pub fn spawn(
    source: Arc<dyn SessionSource>,
    resumer: Arc<dyn Resumer>,
) -> (Requests, mpsc::UnboundedReceiver<Response>) {
    let (search_tx, search_rx) = mpsc::unbounded_channel();
    let (detail_tx, detail_rx) = mpsc::unbounded_channel();
    let (resp_tx, resp_rx) = mpsc::unbounded_channel();
    tokio::spawn(searches(source.clone(), search_rx, resp_tx.clone()));
    tokio::spawn(details(source, resumer, detail_rx, resp_tx));
    (
        Requests {
            searches: search_tx,
            details: detail_tx,
        },
        resp_rx,
    )
}

async fn searches(
    source: Arc<dyn SessionSource>,
    mut requests: mpsc::UnboundedReceiver<Request>,
    responses: mpsc::UnboundedSender<Response>,
) {
    while let Some(mut request) = requests.recv().await {
        // Skip searches that a newer one already superseded.
        while let Ok(next) = requests.try_recv() {
            request = next;
        }
        let Request::Search {
            generation,
            mode,
            filter,
        } = request
        else {
            continue;
        };
        let rows = source.search(&filter).await.map_err(|e| format!("{e:#}"));
        let response = Response::Results {
            generation,
            mode,
            rows,
        };
        if responses.send(response).is_err() {
            return;
        }
    }
}

/// The newest unanswered detail request of each kind.
#[derive(Default)]
struct Latest {
    preview: Option<Request>,
    children: Option<Request>,
    plan: Option<Request>,
    accept: Option<Request>,
    restore: Option<Request>,
    flatten: Option<Request>,
    continuation: Option<Request>,
}

impl Latest {
    /// Keep `request` in place of the one of its kind. Returns the session whose restore it
    /// dropped, if it dropped another session's: the picker asks for a session's restore once,
    /// and waits on it, so it must hear that it won't come.
    #[must_use]
    fn put(&mut self, request: Request) -> Option<HarnessSession> {
        let slot = match request {
            Request::Preview(_) => &mut self.preview,
            Request::Children(_) => &mut self.children,
            Request::Plan(_) => &mut self.plan,
            Request::Accept(_) => &mut self.accept,
            Request::Restore(..) | Request::CatchUp(_) => &mut self.restore,
            Request::Flatten(..) => &mut self.flatten,
            Request::Continue(..) | Request::Fork(..) => &mut self.continuation,
            Request::CancelContinue => {
                self.continuation = None;
                return None;
            }
            Request::Search { .. } => return None,
        };
        let dropped = slot.replace(request)?;
        match (dropped, &*slot) {
            (
                Request::Restore(old, _) | Request::CatchUp(old),
                Some(Request::Restore(new, _) | Request::CatchUp(new)),
            ) if old.handle != new.handle => Some(old.handle),
            _ => None,
        }
    }

    /// The next to answer: a continuation, the restore, then the plan an enter is waiting on (an
    /// enter waits on any of them), then the selection's plan (one may soon be), then what the
    /// chooser shows, then children, then the preview.
    fn take(&mut self) -> Option<Request> {
        self.continuation
            .take()
            .or_else(|| self.restore.take())
            .or_else(|| self.accept.take())
            .or_else(|| self.plan.take())
            .or_else(|| self.flatten.take())
            .or_else(|| self.children.take())
            .or_else(|| self.preview.take())
    }
}

async fn details(
    source: Arc<dyn SessionSource>,
    resumer: Arc<dyn Resumer>,
    mut requests: mpsc::UnboundedReceiver<Request>,
    responses: mpsc::UnboundedSender<Response>,
) {
    let mut latest = Latest::default();
    loop {
        let mut abandoned = Vec::new();
        loop {
            match requests.try_recv() {
                Ok(next) => abandoned.extend(latest.put(next)),
                Err(mpsc::error::TryRecvError::Empty) => break,
                // The picker is gone: nothing queued is wanted any more, and a continuation or
                // restore not yet started must not write a session the user left.
                Err(mpsc::error::TryRecvError::Disconnected) => return,
            }
        }
        for session in abandoned {
            if responses.send(Response::Abandoned(session)).is_err() {
                return;
            }
        }
        let Some(request) = latest.take() else {
            // Nothing is waiting, so this drops nothing.
            match requests.recv().await {
                Some(r) => _ = latest.put(r),
                None => return,
            }
            continue;
        };

        let response = match request {
            Request::Preview(session) => {
                let preview = source.preview(&session).await.unwrap_or_else(|e| {
                    tracing::warn!("failed to load the preview for {session:?}: {e:#}");
                    SessionPreview::default()
                });
                Response::Preview(session, preview)
            }
            Request::Children(session) => {
                let children = source.children(&session).await.unwrap_or_else(|e| {
                    tracing::warn!("failed to load the children of {session:?}: {e:#}");
                    Vec::new()
                });
                Response::Children(session, children)
            }
            Request::Plan(row) | Request::Accept(row) => {
                Response::Plan(row.handle.clone(), resumer.plan(&row).await)
            }
            Request::Restore(row, restore) => Response::Restored(
                row.handle.clone(),
                resumer.restore(source.as_ref(), &row, &restore).await,
            ),
            Request::CatchUp(row) => Response::CaughtUp(
                row.handle.clone(),
                resumer.catch_up(source.as_ref(), &row, None).await,
            ),
            Request::Flatten(session, cwd) => {
                let flattened =
                    source.rehydrate(&session, &cwd).await.map_err(|e| format!("{e:#}")).and_then(
                        |original| continuation::flattened(&original).map_err(|e| e.to_string()),
                    );
                Response::Flattened(session, flattened)
            }
            Request::Continue(row, target, id) => {
                Response::Continued(id, resumer.continue_in(source.as_ref(), &row, target).await)
            }
            Request::Fork(row, id, head) => {
                let source = source.as_ref();
                let from = catchup::fork_from(source, &row.handle, head.as_ref()).await;
                let forked = match from {
                    Ok(from) => resumer.fork(source, &row, from).await,
                    Err(why) => Err(why),
                };
                Response::Forked(id, forked)
            }
            Request::Search { .. } | Request::CancelContinue => continue,
        };
        if responses.send(response).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use atuin_client::ai_session::HarnessKind;
    use parking_lot::Mutex;
    use rstest::rstest;
    use tokio::sync::Semaphore;

    use super::*;
    use crate::resume_tui::fake::{self, FakeResumer, FakeSource};
    use crate::resume_tui::resumer::ForkFrom;

    fn roots() -> SessionFilter {
        let mut filter = SessionFilter::default();
        filter.db.roots_only = true;
        filter
    }

    #[rstest]
    #[tokio::test]
    async fn answers_searches_previews_children_and_plans() {
        let (tx, mut rx) = spawn(Arc::new(FakeSource::new()), Arc::new(FakeResumer::default()));
        tx.send(Request::Search {
            generation: 1,
            mode: FilterMode::Global,
            filter: roots(),
        });
        let Some(Response::Results {
            generation, rows, ..
        }) = rx.recv().await
        else {
            panic!("expected results");
        };
        assert_eq!(generation, 1);
        let rows = rows.unwrap();
        let root = rows.iter().find(|r| r.children == 4).expect("a grouped root");

        tx.send(Request::Children(root.handle.clone()));
        let Some(Response::Children(_, children)) = rx.recv().await else {
            panic!("expected children");
        };
        assert_eq!(children.len(), 1, "the fork, not the subagents");

        tx.send(Request::Preview(root.handle.clone()));
        let Some(Response::Preview(_, preview)) = rx.recv().await else {
            panic!("expected a preview");
        };
        assert!(preview.first_prompt.is_some() && preview.last_assistant.is_some());

        tx.send(Request::Plan(Box::new(root.clone())));
        let Some(Response::Plan(handle, plan)) = rx.recv().await else {
            panic!("expected a plan");
        };
        assert_eq!(handle, root.handle);
        assert_eq!(plan.unwrap().plan.program, "claude");
    }

    /// A source whose previews wait for a permit, recording which sessions were previewed.
    struct SlowPreviews {
        gate: Semaphore,
        started: AtomicUsize,
        previewed: Mutex<Vec<HarnessSession>>,
    }

    #[async_trait]
    impl SessionSource for SlowPreviews {
        async fn search(&self, _: &SessionFilter) -> eyre::Result<Vec<SessionRow>> {
            Ok(vec![fake::row(HarnessKind::ClaudeCode, "found", "t")])
        }

        async fn find_by_id(&self, _: &str) -> eyre::Result<Vec<SessionRow>> {
            Ok(Vec::new())
        }

        async fn preview(&self, session: &HarnessSession) -> eyre::Result<SessionPreview> {
            self.started.fetch_add(1, Ordering::SeqCst);
            self.gate.acquire().await?.forget();
            self.previewed.lock().push(session.clone());
            Ok(SessionPreview::default())
        }

        async fn children(&self, _: &HarnessSession) -> eyre::Result<Vec<SessionRow>> {
            Ok(Vec::new())
        }
    }

    fn session(id: &str) -> HarnessSession {
        fake::row(HarnessKind::ClaudeCode, id, "t").handle
    }

    #[rstest]
    #[tokio::test]
    async fn searches_skip_the_preview_queue_and_passed_over_previews_are_dropped() {
        let source = Arc::new(SlowPreviews {
            gate: Semaphore::new(0),
            started: AtomicUsize::new(0),
            previewed: Mutex::new(Vec::new()),
        });
        let (tx, mut rx) = spawn(source.clone(), Arc::new(FakeResumer::default()));

        // The first preview starts and blocks; the selection then moves over three more rows.
        tx.send(Request::Preview(session("s0")));
        while source.started.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        for id in ["s1", "s2", "s3"] {
            tx.send(Request::Preview(session(id)));
        }
        tx.send(Request::Search {
            generation: 7,
            mode: FilterMode::Global,
            filter: roots(),
        });
        let Some(Response::Results { generation, .. }) = rx.recv().await else {
            panic!("the search should answer while a preview is blocked");
        };
        assert_eq!(generation, 7);

        source.gate.add_permits(10);
        let mut answered = Vec::new();
        for _ in 0..2 {
            let Some(Response::Preview(handle, _)) = rx.recv().await else {
                panic!("expected a preview");
            };
            answered.push(handle);
        }
        assert_eq!(answered, vec![session("s0"), session("s3")]);
        assert_eq!(*source.previewed.lock(), answered);
    }

    /// A [`FakeResumer`] whose continuations and forks wait for a permit, recording the ids
    /// started (a fork's as `fork:<id>`).
    struct GatedContinues {
        inner: FakeResumer,
        gate: Semaphore,
        started: Mutex<Vec<String>>,
    }

    fn gated_continues() -> Arc<GatedContinues> {
        Arc::new(GatedContinues {
            inner: FakeResumer::default(),
            gate: Semaphore::new(0),
            started: Mutex::new(Vec::new()),
        })
    }

    #[async_trait]
    impl Resumer for GatedContinues {
        async fn plan(&self, session: &SessionRow) -> Result<Resume, NotResumable> {
            self.inner.plan(session).await
        }

        async fn restore(
            &self,
            source: &dyn SessionSource,
            session: &SessionRow,
            restore: &Restore,
        ) -> Result<ResumePlan, NotResumable> {
            self.inner.restore(source, session, restore).await
        }

        async fn continue_in(
            &self,
            source: &dyn SessionSource,
            session: &SessionRow,
            target: HarnessKind,
        ) -> Result<Continued, NotResumable> {
            self.started.lock().push(session.handle.session.to_string());
            self.gate.acquire().await.unwrap().forget();
            self.inner.continue_in(source, session, target).await
        }

        async fn fork(
            &self,
            source: &dyn SessionSource,
            session: &SessionRow,
            from: ForkFrom,
        ) -> Result<Forked, NotResumable> {
            self.started.lock().push(format!("fork:{}", session.handle.session));
            self.gate.acquire().await.unwrap().forget();
            self.inner.fork(source, session, from).await
        }
    }

    /// While the worker writes continuation `a`, others are asked for. One a newer one replaced,
    /// or the picker cancelled (it moved on to another choice), before it started never runs:
    /// nothing is written for a choice the user left. `a`, already running, finishes.
    #[rstest]
    #[case::replaced(&["b", "c"], false, &["a", "c"])]
    #[case::cancelled(&["b"], true, &["a"])]
    #[case::replaced_then_cancelled(&["b", "c"], true, &["a"])]
    #[tokio::test]
    async fn a_superseded_continuation_never_starts(
        #[case] queued: &[&str],
        #[case] cancel: bool,
        #[case] expected: &[&str],
    ) {
        let resumer = gated_continues();
        let (tx, mut rx) = spawn(Arc::new(FakeSource::new()), resumer.clone());
        let continue_ = |id: &str, n: u64| {
            let row = fake::row(HarnessKind::ClaudeCode, id, "t");
            Request::Continue(Box::new(row), HarnessKind::Codex, n)
        };
        tx.send(continue_("a", 0));
        while resumer.started.lock().is_empty() {
            tokio::task::yield_now().await;
        }
        for (n, id) in (1..).zip(queued) {
            tx.send(continue_(id, n));
        }
        if cancel {
            tx.send(Request::CancelContinue);
        }
        // Answered after any continuation still kept, which goes first.
        tx.send(Request::Preview(session("after")));
        resumer.gate.add_permits(10);

        loop {
            let next = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv());
            match next.await.expect("the worker is stuck").expect("the worker stopped") {
                Response::Continued(_, result) => assert!(result.is_ok(), "{result:?}"),
                Response::Preview(..) => break,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(*resumer.started.lock(), expected);
    }

    /// A fork waits in a continuation's place: one asked for while continuation `a` is written
    /// never starts when the picker cancels it, or a newer continuation replaces it; and a newer
    /// fork replaces a waiting continuation.
    #[rstest]
    #[case::fork_cancelled(&[true], true, &["a"])]
    #[case::fork_replaced(&[true, false], false, &["a", "c"])]
    #[case::fork_replaces(&[false, true], false, &["a", "fork:c"])]
    #[tokio::test]
    async fn a_superseded_fork_never_starts(
        #[case] forks: &[bool],
        #[case] cancel: bool,
        #[case] expected: &[&str],
    ) {
        let resumer = gated_continues();
        let (tx, mut rx) = spawn(Arc::new(FakeSource::new()), resumer.clone());
        let row = |id: &str| Box::new(fake::row(HarnessKind::ClaudeCode, id, "t"));
        tx.send(Request::Continue(row("a"), HarnessKind::Codex, 0));
        while resumer.started.lock().is_empty() {
            tokio::task::yield_now().await;
        }
        // `b`, then `c`: each forked, or continued.
        for ((n, id), &fork) in (1..).zip(["b", "c"]).zip(forks) {
            tx.send(if fork {
                Request::Fork(row(id), n, None)
            } else {
                Request::Continue(row(id), HarnessKind::Codex, n)
            });
        }
        if cancel {
            tx.send(Request::CancelContinue);
        }
        tx.send(Request::Preview(session("after")));
        resumer.gate.add_permits(10);

        loop {
            let next = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv());
            match next.await.expect("the worker is stuck").expect("the worker stopped") {
                Response::Continued(_, result) => assert!(result.is_ok(), "{result:?}"),
                Response::Forked(_, result) => assert!(result.is_ok(), "{result:?}"),
                Response::Preview(..) => break,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(*resumer.started.lock(), expected);
    }

    /// The picker gone (its requests dropped) while a continuation waits behind a running one:
    /// the waiting one never runs.
    #[rstest]
    #[tokio::test]
    async fn nothing_queued_runs_once_the_picker_is_gone() {
        let resumer = gated_continues();
        let (tx, mut rx) = spawn(Arc::new(FakeSource::new()), resumer.clone());
        let row = |id: &str| Box::new(fake::row(HarnessKind::ClaudeCode, id, "t"));
        tx.send(Request::Continue(row("a"), HarnessKind::Codex, 0));
        while resumer.started.lock().is_empty() {
            tokio::task::yield_now().await;
        }
        tx.send(Request::Continue(row("b"), HarnessKind::Codex, 1));
        drop(tx);
        resumer.gate.add_permits(10);

        // `a` finishes; then the worker stops, its response channel closing.
        let next = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while rx.recv().await.is_some() {}
        });
        next.await.expect("the worker kept running");
        assert_eq!(*resumer.started.lock(), ["a"]);
    }

    /// An enter on a fork while the detail lane is busy, and then the selection settling on the
    /// root the fork is under: the root's plan, asked for after the fork's, doesn't take its
    /// place, so the plan the enter waits on still comes.
    #[rstest]
    #[tokio::test]
    async fn a_later_plan_for_another_session_never_drops_an_accept() {
        let source = Arc::new(SlowPreviews {
            gate: Semaphore::new(0),
            started: AtomicUsize::new(0),
            previewed: Mutex::new(Vec::new()),
        });
        let (tx, mut rx) = spawn(source.clone(), Arc::new(FakeResumer::default()));
        let root = fake::row(HarnessKind::ClaudeCode, "root", "t");
        let fork = fake::row(HarnessKind::ClaudeCode, "fork", "t");

        // The lane is busy with a preview; meanwhile the root's plan is asked for, then the
        // fork's for an enter, then the root's again as the selection settles back on it.
        tx.send(Request::Preview(root.handle.clone()));
        while source.started.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        tx.send(Request::Plan(Box::new(root.clone())));
        tx.send(Request::Accept(Box::new(fork.clone())));
        tx.send(Request::Plan(Box::new(root.clone())));
        source.gate.add_permits(10);

        let mut planned = Vec::new();
        while planned.len() < 2 {
            let next = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv());
            match next.await.expect("a plan was dropped").expect("the worker stopped") {
                Response::Plan(handle, plan) => {
                    assert!(plan.is_ok(), "{plan:?}");
                    planned.push(handle);
                }
                Response::Preview(..) => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        // The accept's first: an enter is waiting on it.
        assert_eq!(planned, vec![fork.handle, root.handle]);
    }
}
