use std::sync::Arc;

use atuin_client::ai_session::{Appended, HarnessKind, NativeSessionId};
use atuin_common::harnesstools::AnyHarness;
use atuin_common::harnesstools::session::any::AnySessions;
use futures::{Stream, StreamExt};

use super::Sink;
use super::normalizer::Normalizer;

#[derive(Debug, Clone)]
pub enum ImportProgress {
    Session {
        harness: HarnessKind,
        session: NativeSessionId,
        imported: u64,
        skipped: u64,
        failed: u64,
    },
    Finished {
        sessions: u64,
        imported: u64,
        skipped: u64,
        failed: u64,
    },
}

pub struct SessionImporter {
    sink: Arc<Sink>,
    concurrency: usize,
}

impl SessionImporter {
    pub fn new(sink: Arc<Sink>) -> Self {
        Self {
            sink,
            concurrency: 8,
        }
    }

    pub fn run(
        self,
        filter: Option<HarnessKind>,
    ) -> impl Stream<Item = ImportProgress> + Send + 'static {
        async_stream::stream! {
            let mut sessions = 0u64;
            let mut imported = 0u64;
            let mut skipped = 0u64;
            let mut failed = 0u64;
            for harness in AnyHarness::all() {
                let kind = HarnessKind::from(harness);
                if filter.is_some_and(|want| want != kind) {
                    continue;
                }
                let Some(observed) = harness.sessions() else {
                    continue;
                };
                let progress = self.harness(kind, observed);
                futures::pin_mut!(progress);
                while let Some(event) = progress.next().await {
                    if let ImportProgress::Session { imported: i, skipped: s, failed: f, .. } =
                        &event
                    {
                        sessions += 1;
                        imported += i;
                        skipped += s;
                        failed += f;
                    }
                    yield event;
                }
            }
            yield ImportProgress::Finished { sessions, imported, skipped, failed };
        }
    }

    pub fn harness(
        &self,
        kind: HarnessKind,
        sessions: AnySessions,
    ) -> impl Stream<Item = ImportProgress> + Send + 'static {
        let sink = self.sink.clone();
        let concurrency = self.concurrency;
        async_stream::stream! {
            let Ok(existing) = sessions.existing() else {
                return;
            };
            let mut imports = existing
                .map(|session| {
                    let sink = sink.clone();
                    async move {
                        // One enricher per session: it carries that session's bookkeeping (title,
                        // timestamps, parent, usage dedupe) across its lines, exactly as live
                        // capture does, so a backfilled row matches the captured one.
                        let mut normalizer = Normalizer::new(kind);
                        let sid = session.id();
                        let mut imported = 0u64;
                        let mut skipped = 0u64;
                        let mut failed = 0u64;
                        let mut messages = session.read();
                        while let Some(next) = messages.next().await {
                            let Ok(message) = next else {
                                failed += 1;
                                continue;
                            };
                            // A bookkeeping line worth no row (matches live capture) is not
                            // counted: it is neither a new record nor a dedupe skip.
                            let Some(msg) = normalizer.enrich(&sid, &message) else {
                                continue;
                            };
                            match sink.append(msg).await {
                                Ok(Appended::New) => imported += 1,
                                Ok(Appended::Duplicate) => skipped += 1,
                                Err(_) => failed += 1,
                            }
                        }
                        ImportProgress::Session {
                            harness: kind,
                            session: normalizer.handle(&sid).session,
                            imported,
                            skipped,
                            failed,
                        }
                    }
                })
                .buffer_unordered(concurrency);
            while let Some(event) = imports.next().await {
                yield event;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use atuin_client::ai_session::{
        AiSessionDatabase, AiSessionStore, HarnessSession, NativeSessionId,
    };
    use atuin_client::record::sqlite_store::SqliteStore;
    use atuin_common::encryption::paseto_v4::Key;
    use atuin_common::harnesstools::pi::session::PiSessions;
    use atuin_domain::record::HostId;
    use futures::StreamExt;
    use rstest::rstest;

    use super::*;
    use crate::session_capture::AiHarnessSessionCapture;

    async fn mem_store() -> AiSessionStore {
        let store = SqliteStore::in_memory(std::time::Duration::from_secs(5)).await.unwrap();
        AiSessionStore::builder()
            .store(store)
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build()
    }

    fn write_pi_session(root: &Path, id: &str, turns: &[&str]) {
        let body = turns
            .iter()
            .enumerate()
            .map(|(i, turn)| {
                let (role, text) = turn.split_once(':').unwrap();
                serde_json::json!({
                    "type": "message",
                    "id": format!("{id}-m{i}"),
                    "message": {"role": role, "content": text},
                })
                .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
            // Trailing newline: read() reads complete lines only (like live capture), so a real
            // session terminates its final record.
            + "\n";
        std::fs::write(root.join(format!("1700000000_{id}.jsonl")), body).unwrap();
    }

    fn pi_sessions(root: &Path) -> AnySessions {
        AnySessions::from(PiSessions::builder().root(root.to_path_buf()).build())
    }

    fn sum_new(progress: &[ImportProgress]) -> u64 {
        progress
            .iter()
            .map(|event| match event {
                ImportProgress::Session { imported, .. } => *imported,
                ImportProgress::Finished { .. } => 0,
            })
            .sum()
    }

    fn sum_skipped(progress: &[ImportProgress]) -> u64 {
        progress
            .iter()
            .map(|event| match event {
                ImportProgress::Session { skipped, .. } => *skipped,
                ImportProgress::Finished { .. } => 0,
            })
            .sum()
    }

    #[rstest]
    #[tokio::test]
    async fn import_is_idempotent_and_accounts_new_vs_skipped() {
        let root = tempfile::tempdir().unwrap();
        write_pi_session(root.path(), "s1", &["user:hi", "assistant:yo"]);

        let sink =
            Arc::new(Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap()));
        let handle = HarnessSession {
            harness: HarnessKind::Pi,
            session: NativeSessionId::from("s1".to_owned()),
        };

        let first: Vec<_> = SessionImporter::new(sink.clone())
            .harness(HarnessKind::Pi, pi_sessions(root.path()))
            .collect()
            .await;
        assert_eq!(sum_new(&first), 2);
        assert_eq!(sum_skipped(&first), 0);
        let after_first = sink.sidecar.get_session(&handle).await.unwrap().unwrap().message_count;
        assert_eq!(after_first, 2);

        let second: Vec<_> = SessionImporter::new(sink.clone())
            .harness(HarnessKind::Pi, pi_sessions(root.path()))
            .collect()
            .await;
        assert_eq!(sum_new(&second), 0);
        assert_eq!(sum_skipped(&second), 2);
        let after_second = sink.sidecar.get_session(&handle).await.unwrap().unwrap().message_count;
        assert_eq!(after_second, 2);
    }

    #[rstest]
    #[tokio::test]
    async fn nop_facade_import_emits_a_single_empty_summary() {
        let cap = AiHarnessSessionCapture::nop().await;
        let events: Vec<_> = cap.import(None).collect().await;
        assert!(matches!(events.as_slice(), [ImportProgress::Finished {
            sessions: 0,
            imported: 0,
            skipped: 0,
            failed: 0
        }]));
    }
}
