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
}

/// Start the worker. It stops when the request sender is dropped.
pub fn spawn(
    source: Arc<dyn SessionSource>,
) -> (mpsc::UnboundedSender<Request>, mpsc::UnboundedReceiver<Response>) {
    let (req_tx, req_rx) = mpsc::unbounded_channel();
    let (resp_tx, resp_rx) = mpsc::unbounded_channel();
    tokio::spawn(run(source, req_rx, resp_tx));
    (req_tx, resp_rx)
}

async fn run(
    source: Arc<dyn SessionSource>,
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
        };
        if responses.send(response).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resume_tui::fake::FakeSource;

    #[tokio::test]
    async fn answers_searches_previews_and_children() {
        let (tx, mut rx) = spawn(Arc::new(FakeSource::new()));
        tx.send(Request::Search {
            generation: 1,
            mode: FilterMode::Global,
            filter: SessionFilter {
                group_forks: true,
                include_subagents: true,
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
        let root = rows.iter().find(|r| r.children == 3).expect("a grouped root");

        tx.send(Request::Children {
            session: root.handle.clone(),
            include_subagents: true,
        })
        .unwrap();
        let Some(Response::Children(_, children)) = rx.recv().await else {
            panic!("expected children");
        };
        assert_eq!(children.len(), 3);

        tx.send(Request::Preview(root.handle.clone())).unwrap();
        let Some(Response::Preview(_, preview)) = rx.recv().await else {
            panic!("expected a preview");
        };
        assert!(preview.first_prompt.is_some() && preview.last_assistant.is_some());
    }
}
