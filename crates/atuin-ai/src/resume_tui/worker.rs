//! Runs source queries off the UI thread.
//!
//! The UI sends requests tagged with a generation; the worker coalesces queued searches (only the
//! newest matters while typing) and answers on a channel. The UI drops answers for any generation
//! but the newest, and keeps the old list on screen until then.

use std::collections::VecDeque;
use std::sync::Arc;

use atuin_client::ai_session::HarnessSession;
use atuin_client::settings::AiSessionFilterMode as FilterMode;
use tokio::sync::mpsc;

use super::resumer::{NotResumable, ResumePlan, Resumer};
use super::source::{SessionFilter, SessionPreview, SessionRow, SessionSource};

#[derive(Debug)]
pub enum Request {
    Search {
        generation: u64,
        mode: FilterMode,
        filter: SessionFilter,
    },
    Preview(HarnessSession),
    Children {
        session: HarnessSession,
        include_subagents: bool,
    },
    /// Plan resuming a session (may walk the harness's session directories).
    Plan(Box<SessionRow>),
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
    Plan(HarnessSession, Result<ResumePlan, NotResumable>),
}

/// Start the worker. It stops when the request sender is dropped.
pub fn spawn(
    source: Arc<dyn SessionSource>,
    resumer: Arc<dyn Resumer>,
) -> (mpsc::UnboundedSender<Request>, mpsc::UnboundedReceiver<Response>) {
    let (req_tx, req_rx) = mpsc::unbounded_channel();
    let (resp_tx, resp_rx) = mpsc::unbounded_channel();
    tokio::spawn(run(source, resumer, req_rx, resp_tx));
    (req_tx, resp_rx)
}

async fn run(
    source: Arc<dyn SessionSource>,
    resumer: Arc<dyn Resumer>,
    mut requests: mpsc::UnboundedReceiver<Request>,
    responses: mpsc::UnboundedSender<Response>,
) {
    let mut backlog = VecDeque::new();
    loop {
        let request = match backlog.pop_front() {
            Some(r) => r,
            None => match requests.recv().await {
                Some(r) => r,
                None => return,
            },
        };

        // Skip searches that a newer one already superseded.
        let request = if matches!(request, Request::Search { .. }) {
            let mut latest = request;
            while let Ok(next) = requests.try_recv() {
                if matches!(next, Request::Search { .. }) {
                    latest = next;
                } else {
                    backlog.push_back(next);
                }
            }
            latest
        } else {
            request
        };

        let response = match request {
            Request::Search {
                generation,
                mode,
                filter,
            } => Response::Results {
                generation,
                mode,
                rows: source.search(&filter).await.map_err(|e| format!("{e:#}")),
            },
            Request::Preview(session) => {
                let preview = source.preview(&session).await.unwrap_or_else(|e| {
                    tracing::warn!("failed to load the preview for {session:?}: {e:#}");
                    SessionPreview::default()
                });
                Response::Preview(session, preview)
            }
            Request::Children {
                session,
                include_subagents,
            } => {
                let children =
                    source.children(&session, include_subagents).await.unwrap_or_else(|e| {
                        tracing::warn!("failed to load the children of {session:?}: {e:#}");
                        Vec::new()
                    });
                Response::Children(session, children)
            }
            Request::Plan(row) => Response::Plan(row.handle.clone(), resumer.plan(&row).await),
        };
        if responses.send(response).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::resume_tui::fake::{FakeResumer, FakeSource};

    #[rstest]
    #[tokio::test]
    async fn answers_searches_previews_children_and_plans() {
        let (tx, mut rx) = spawn(Arc::new(FakeSource::new()), Arc::new(FakeResumer::default()));
        tx.send(Request::Search {
            generation: 1,
            mode: FilterMode::Global,
            filter: SessionFilter {
                roots_only: true,
                ..SessionFilter::default()
            },
        })
        .unwrap();
        let Some(Response::Results {
            generation, rows, ..
        }) = rx.recv().await
        else {
            panic!("expected results");
        };
        assert_eq!(generation, 1);
        let rows = rows.unwrap();
        let root = rows.iter().find(|r| r.children == 4).expect("a grouped root");

        tx.send(Request::Children {
            session: root.handle.clone(),
            include_subagents: true,
        })
        .unwrap();
        let Some(Response::Children(_, children)) = rx.recv().await else {
            panic!("expected children");
        };
        assert_eq!(children.len(), 4);

        tx.send(Request::Preview(root.handle.clone())).unwrap();
        let Some(Response::Preview(_, preview)) = rx.recv().await else {
            panic!("expected a preview");
        };
        assert!(preview.first_prompt.is_some() && preview.last_assistant.is_some());

        tx.send(Request::Plan(Box::new(root.clone()))).unwrap();
        let Some(Response::Plan(handle, plan)) = rx.recv().await else {
            panic!("expected a plan");
        };
        assert_eq!(handle, root.handle);
        assert_eq!(plan.unwrap().program, "claude");
    }
}
