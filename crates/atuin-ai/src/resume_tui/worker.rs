//! Runs source queries off the UI thread.
//!
//! Two lanes, so a search never waits behind the selected session's details:
//! - searches, tagged with a generation. Queued ones are coalesced (only the newest matters while
//!   typing), and the UI drops answers for any generation but the newest, keeping the old list on
//!   screen until then;
//! - details (preview, children, plan) for the selected session. Only the newest request of each
//!   kind is kept: while the selection moves, the sessions it passed over are never loaded.
//!   Catching a session up with sync (or restoring it) to resume it, or continuing it in another
//!   harness, which only an enter or tab asks for, goes first.
//!
//! Host names are read once beside them, so rows listed before they are known can be relabelled.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use atuin_client::ai_session::{HarnessKind, HarnessSession, SessionHeads, SourceId};
use atuin_client::settings::AiSessionFilterMode as FilterMode;
use atuin_common::harnesstools::continuation::{self, Flattened};
use tokio::sync::mpsc;

use super::catchup::Synced;
use super::resumer::{Continued, NotResumable, Resume, Resumer};
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
    /// Bring this machine's copy of a session to a head (the newest when `None`), restoring it
    /// from sync when it isn't here, and plan resuming it ([`Resumer::sync`]).
    Sync(Box<SessionRow>, Option<SourceId>),
    /// Read a session's heads as they are now (the row's are from the last search).
    Heads(HarnessSession),
    /// Count what continuing a session in another harness would flatten (reads all of it, or the
    /// branch ending at the head given).
    Flatten(HarnessSession, Option<SourceId>, PathBuf),
    /// Write a session (the branch ending at the head given, else its newest) out as a new
    /// session of another harness, and plan resuming that.
    Continue(Box<SessionRow>, HarnessKind, Option<SourceId>),
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
    /// The session is caught up to a head (or couldn't be): the plan that resumes it, and what
    /// was done.
    Synced(HarnessSession, Option<SourceId>, Result<Synced, NotResumable>),
    Flattened(HarnessSession, Option<SourceId>, Result<Flattened, String>),
    /// A session's heads as they are now; `None` when they can't be read.
    Heads(HarnessSession, Option<SessionHeads>),
    /// The session is continued in another harness (or couldn't be).
    Continued(HarnessSession, Result<Continued, NotResumable>),
    /// Other hosts' names, by host id (see [`SessionSource::host_names`]).
    HostNames(HashMap<String, String>),
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
    tokio::spawn(host_names(source.clone(), resp_tx.clone()));
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

async fn host_names(source: Arc<dyn SessionSource>, responses: mpsc::UnboundedSender<Response>) {
    match source.host_names().await {
        Ok(names) if !names.is_empty() => {
            let _ = responses.send(Response::HostNames(names));
        }
        Ok(_) => {}
        Err(e) => tracing::debug!("no host names: {e:#}"),
    }
}

/// The newest unanswered detail request of each kind.
#[derive(Default)]
struct Latest {
    preview: Option<Request>,
    children: Option<Request>,
    plan: Option<Request>,
    heads: Option<Request>,
    sync: Option<Request>,
    flatten: Option<Request>,
    continuation: Option<Request>,
}

impl Latest {
    fn put(&mut self, request: Request) {
        let slot = match request {
            Request::Preview(_) => &mut self.preview,
            Request::Children(_) => &mut self.children,
            Request::Plan(_) => &mut self.plan,
            Request::Heads(_) => &mut self.heads,
            Request::Sync(..) => &mut self.sync,
            Request::Flatten(..) => &mut self.flatten,
            Request::Continue(..) => &mut self.continuation,
            Request::Search { .. } => return,
        };
        *slot = Some(request);
    }

    /// The next to answer: a continuation, a catch-up, heads or a plan first (an enter may be
    /// waiting on it), then what the chooser shows, then children, then the preview.
    fn take(&mut self) -> Option<Request> {
        self.continuation
            .take()
            .or_else(|| self.sync.take())
            .or_else(|| self.heads.take())
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
        while let Ok(next) = requests.try_recv() {
            latest.put(next);
        }
        let Some(request) = latest.take() else {
            match requests.recv().await {
                Some(r) => latest.put(r),
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
            Request::Plan(row) => Response::Plan(row.handle.clone(), resumer.plan(&row).await),
            Request::Heads(session) => {
                let heads = source.heads(&session).await.unwrap_or_else(|e| {
                    tracing::warn!("failed to read the heads of {session:?}: {e:#}");
                    None
                });
                Response::Heads(session, heads)
            }
            Request::Sync(row, head) => {
                let synced = resumer.sync(source.as_ref(), &row, head.as_ref()).await;
                Response::Synced(row.handle.clone(), head, synced)
            }
            Request::Flatten(session, head, cwd) => {
                let original = match &head {
                    Some(head) => source.branch(&session, head, &cwd).await,
                    None => source.rehydrate(&session, &cwd).await,
                };
                let flattened = original
                    .map(|original| continuation::flattened(&original))
                    .map_err(|e| format!("{e:#}"));
                Response::Flattened(session, head, flattened)
            }
            Request::Continue(row, target, head) => Response::Continued(
                row.handle.clone(),
                resumer.continue_in(source.as_ref(), &row, target, head.as_ref()).await,
            ),
            Request::Search { .. } => continue,
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

    fn roots() -> SessionFilter {
        SessionFilter {
            roots_only: true,
            ..SessionFilter::default()
        }
    }

    /// The next response but host names, which come whenever they are read.
    async fn next(rx: &mut mpsc::UnboundedReceiver<Response>) -> Option<Response> {
        loop {
            match rx.recv().await {
                Some(Response::HostNames(_)) => {}
                other => return other,
            }
        }
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
        }) = next(&mut rx).await
        else {
            panic!("expected results");
        };
        assert_eq!(generation, 1);
        let rows = rows.unwrap();
        let root = rows.iter().find(|r| r.children == 4).expect("a grouped root");

        tx.send(Request::Children(root.handle.clone()));
        let Some(Response::Children(_, children)) = next(&mut rx).await else {
            panic!("expected children");
        };
        assert_eq!(children.len(), 1, "the fork, not the subagents");

        tx.send(Request::Preview(root.handle.clone()));
        let Some(Response::Preview(_, preview)) = next(&mut rx).await else {
            panic!("expected a preview");
        };
        assert!(preview.first_prompt.is_some() && preview.last_assistant.is_some());

        tx.send(Request::Plan(Box::new(root.clone())));
        let Some(Response::Plan(handle, plan)) = next(&mut rx).await else {
            panic!("expected a plan");
        };
        assert_eq!(handle, root.handle);
        assert_eq!(plan.unwrap().plan.program, "claude");
    }

    /// A source whose host names wait for a permit: they come after the first search answers.
    struct SlowNames(Semaphore);

    #[async_trait]
    impl SessionSource for SlowNames {
        async fn search(&self, _: &SessionFilter) -> eyre::Result<Vec<SessionRow>> {
            Ok(Vec::new())
        }

        async fn find_by_id(&self, _: &str) -> eyre::Result<Vec<SessionRow>> {
            Ok(Vec::new())
        }

        async fn preview(&self, _: &HarnessSession) -> eyre::Result<SessionPreview> {
            Ok(SessionPreview::default())
        }

        async fn children(&self, _: &HarnessSession) -> eyre::Result<Vec<SessionRow>> {
            Ok(Vec::new())
        }

        async fn host_names(&self) -> eyre::Result<HashMap<String, String>> {
            self.0.acquire().await?.forget();
            Ok(HashMap::from([("h".to_owned(), "buildbox".to_owned())]))
        }
    }

    #[rstest]
    #[tokio::test]
    async fn host_names_never_hold_up_a_search() {
        let source = Arc::new(SlowNames(Semaphore::new(0)));
        let (tx, mut rx) = spawn(source.clone(), Arc::new(FakeResumer::default()));
        tx.send(Request::Search {
            generation: 1,
            mode: FilterMode::Global,
            filter: roots(),
        });
        assert!(matches!(rx.recv().await, Some(Response::Results { generation: 1, .. })));

        source.0.add_permits(1);
        let Some(Response::HostNames(names)) = rx.recv().await else {
            panic!("expected host names");
        };
        assert_eq!(names["h"], "buildbox");
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
}
