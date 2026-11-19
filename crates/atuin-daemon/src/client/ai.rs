//! The client for the daemon's AI session service.

use atuin_client::ai_session::{HarnessKind, HarnessSession, SearchTerms, SessionFilter};
use atuin_client::settings::Settings;
use eyre::Result;
use jiff::Timestamp;
use tonic::transport::Channel;

use super::{DaemonChannel, FromSettingsError};
use crate::grpc::ai::agent::pb::{HarnessSession as AgentHarnessSession, Session as AiSession};
use crate::grpc::ai::session::pb::ai_session_client::AiSessionClient as AiSessionServiceClient;
use crate::grpc::ai::session::pb::{
    GetSessionEvent, GetSessionRequest, ImportSessionsEvent, ImportSessionsRequest,
    ListSessionsRequest, ProjectSyncedRequest, PushMessagesReply, PushMessagesRequest,
    RebuildSessionsRequest, SearchSessionsMatch, SearchSessionsRequest, TailSessionsEvent,
    TailSessionsRequest,
};
use crate::grpc::request::ClientInterceptor;
use crate::service::ActiveService;
use crate::service::address::{ConnectError, DaemonAddress};
use crate::session_capture::Origin;

/// First and longest waits between [`AiClient::wait_for_sessions`] probes.
const REBUILD_POLL_START: std::time::Duration = std::time::Duration::from_millis(250);
const REBUILD_POLL_MAX: std::time::Duration = std::time::Duration::from_secs(5);

/// Client for the daemon's `ai.session.AiSession` service. Wraps the generated tonic stub the same
/// way [`HistoryClient`](super::HistoryClient) and [`SearchClient`](super::SearchClient) do,
/// returning the raw protobuf messages so callers can render them however they like.
pub struct AiClient {
    client: AiSessionServiceClient<DaemonChannel>,
}

impl AiClient {
    /// The stub over `channel`, taking messages of any size: a message the daemon streams holds
    /// one captured message, as big as the prompt or reply in it, past tonic's default 4 MiB.
    /// The daemon is local and trusted (the server encodes any size by default).
    fn service(channel: Channel) -> AiSessionServiceClient<DaemonChannel> {
        AiSessionServiceClient::with_interceptor(channel, ClientInterceptor)
            .max_decoding_message_size(usize::MAX)
    }

    pub async fn new(address: impl Into<DaemonAddress>) -> Result<Self, ConnectError> {
        Ok(Self {
            client: Self::service(address.into().channel().await?),
        })
    }

    pub async fn from_settings(settings: &Settings) -> Result<Self, FromSettingsError> {
        Ok(Self::new(ActiveService::locate(&settings.daemon)?).await?)
    }

    /// Wait while the daemon is still rebuilding AI sessions after starting: until then every
    /// session read but a tail is refused rather than answered partially. Calls `on_wait` each
    /// time it finds the daemon rebuilding, with how far it has got (records replayed, and
    /// roughly how many there are to replay), and returns at once when it is not. After this the
    /// daemon serves reads until a store command has it rebuild again (`atuin store rebuild
    /// ai-session`, a purge or a forced pull), which is rare.
    pub async fn wait_for_sessions(&mut self, mut on_wait: impl FnMut((u64, u64))) -> Result<()> {
        let mut delay = REBUILD_POLL_START;
        while let Some(progress) = self.rebuild_status().await? {
            on_wait(progress);
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(REBUILD_POLL_MAX);
        }
        Ok(())
    }

    /// Whether the daemon is rebuilding AI sessions right now (see [`Self::wait_for_sessions`]),
    /// without waiting for it to finish: `Some` while it is, with how far it has got (records
    /// replayed, and roughly how many there are to replay); `None` when it serves reads.
    pub async fn rebuild_status(&mut self) -> Result<Option<(u64, u64)>> {
        // A listing filtered to the future matches nothing, so the probe costs little beyond the
        // rebuild check every read makes first.
        let future = Timestamp::now() + jiff::SignedDuration::from_hours(24 * 365);
        let filter = SessionFilter {
            updated_since: Some(future),
            ..SessionFilter::default()
        };
        let probe = ListSessionsRequest {
            filter: Some((&filter).into()),
        };
        match self.client.list_sessions(probe).await {
            Ok(_) => Ok(None),
            Err(status) => match crate::grpc::ai::session::rebuild_progress(&status) {
                Some(progress) => Ok(Some(progress)),
                None => Err(status.into()),
            },
        }
    }

    /// Stream captured session summaries passing `filter`, newest first. The daemon sends one
    /// session per message (so a long list never trips the gRPC message-size limit); callers that
    /// want the whole set collect it with `try_collect`, and ones that only want the newest can
    /// take the first item without draining the rest.
    pub async fn list_sessions(
        &mut self,
        filter: &SessionFilter,
    ) -> Result<tonic::Streaming<AiSession>> {
        let request = ListSessionsRequest {
            filter: Some(filter.into()),
        };
        Ok(self.client.list_sessions(request).await?.into_inner())
    }

    /// Stream one session: the first event carries the [`AiSession`], each event after it a message.
    pub async fn get_session(
        &mut self,
        session: HarnessSession,
    ) -> Result<tonic::Streaming<GetSessionEvent>> {
        let request = GetSessionRequest {
            session: Some(session.into()),
        };
        Ok(self.client.get_session(request).await?.into_inner())
    }

    /// Follow sessions and messages as they are recorded. `harness` filters to one harness when set.
    pub async fn tail_sessions(
        &mut self,
        harness: Option<HarnessKind>,
    ) -> Result<tonic::Streaming<TailSessionsEvent>> {
        let request = TailSessionsRequest {
            harness: harness.map(|h| h as i32),
        };
        Ok(self.client.tail_sessions(request).await?.into_inner())
    }

    /// Stream the sessions matching `terms` and passing `filter`, most relevant first, at most
    /// `limit` (0 is unbounded). No terms stream them newest first.
    ///
    /// The daemon matches every term as whole words, or with [`SearchTerms::Any`] any term as a
    /// prefix; a [`SearchTerms::Typed`] line (search as you type) is searched as
    /// [`SearchTerms::All`] over its words.
    pub async fn search_sessions(
        &mut self,
        terms: SearchTerms,
        filter: &SessionFilter,
        limit: u32,
    ) -> Result<tonic::Streaming<SearchSessionsMatch>> {
        let (terms, any_term) = match terms {
            SearchTerms::All(terms) => (terms, false),
            SearchTerms::Typed(line) => {
                (line.split_whitespace().map(str::to_owned).collect(), false)
            }
            SearchTerms::Any(terms) => (terms, true),
        };
        let request = SearchSessionsRequest {
            terms,
            limit,
            any_term,
            filter: Some(filter.into()),
        };
        Ok(self.client.search_sessions(request).await?.into_inner())
    }

    /// Have the daemon rebuild its sessions from the record store, after records were deleted
    /// under them. Returns once what they projected is gone; the replay goes on in the
    /// background, and [`Self::wait_for_sessions`] waits it out.
    pub async fn rebuild_sessions(&mut self) -> Result<()> {
        self.client.rebuild_sessions(RebuildSessionsRequest {}).await?;
        Ok(())
    }

    /// Have the daemon project the records this process synced into the record store. Returns
    /// once the projection is over, which goes on if this stops waiting; a failure is only in the
    /// daemon's log.
    pub async fn project_synced(&mut self) -> Result<()> {
        self.client.project_synced(ProjectSyncedRequest {}).await?;
        Ok(())
    }

    /// Hand the daemon messages of `session`, each a line of `atuin threads push` input, to
    /// capture: one push, sent a batch at a time.
    pub async fn push_messages(
        &mut self,
        session: HarnessSession,
        batches: Vec<Vec<String>>,
        origin: Origin,
    ) -> Result<PushMessagesReply> {
        let session = AgentHarnessSession::from(session);
        let requests = batches.into_iter().map(move |lines| PushMessagesRequest {
            session: Some(session.clone()),
            lines,
            imported: origin == Origin::Import,
        });
        Ok(self.client.push_messages(futures::stream::iter(requests)).await?.into_inner())
    }

    pub async fn import_sessions(
        &mut self,
        harness: Option<HarnessKind>,
    ) -> Result<tonic::Streaming<ImportSessionsEvent>> {
        let request = ImportSessionsRequest {
            harness: harness.map(|h| h as i32),
            background: false,
        };
        Ok(self.client.import_sessions(request).await?.into_inner())
    }

    /// Have the daemon import every harness's sessions after replying.
    pub async fn import_sessions_in_background(&mut self) -> Result<()> {
        let request = ImportSessionsRequest {
            harness: None,
            background: true,
        };
        self.client.import_sessions(request).await?;
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use rstest::rstest;
    use tokio::net::UnixListener;
    use tokio::sync::watch;
    use tokio_stream::wrappers::UnixListenerStream;
    use tonic::transport::Server;

    use super::AiClient;
    use crate::grpc::AiSessionService;
    use crate::grpc::ai::session::pb::ai_session_server::AiSessionServer;
    use crate::session_capture::{AiHarnessSessionCapture, Origin, StoreState};

    /// A client of an AI-session service over a real socket, in the given recovery state.
    async fn serve(state: StoreState) -> (AiClient, watch::Sender<StoreState>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.sock");
        let (capture, state) = AiHarnessSessionCapture::with_state(state).await;
        let service = AiSessionServer::new(AiSessionService::new(Arc::new(capture)));
        let incoming = UnixListenerStream::new(UnixListener::bind(&path).unwrap());
        tokio::spawn(Server::builder().add_service(service).serve_with_incoming(incoming));
        (AiClient::new(path).await.unwrap(), state, dir)
    }

    /// A push whose later batch is bad still stores what the batches before it held, a line
    /// waiting for a timed one included.
    #[rstest]
    #[tokio::test]
    async fn a_bad_batch_keeps_the_batches_before_it() {
        use atuin_client::ai_session::{HarnessKind, HarnessSession};
        use futures::StreamExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.sock");
        let (capture, _state) = AiHarnessSessionCapture::with_state(StoreState::Ready).await;
        let capture = Arc::new(capture);
        let service = AiSessionServer::new(AiSessionService::new(capture.clone()));
        let incoming = UnixListenerStream::new(UnixListener::bind(&path).unwrap());
        tokio::spawn(Server::builder().add_service(service).serve_with_incoming(incoming));
        let mut client = AiClient::new(path).await.unwrap();

        let handle = HarnessSession {
            harness: HarnessKind::Amp,
            session: "T-1".to_owned().into(),
        };
        let untimed = r#"{"v":1,"id":"m1","role":"user","content":[{"type":"text","text":"hi"}]}"#;
        let batches = vec![vec![untimed.to_owned()], vec!["not a message".to_owned()]];
        assert!(client.push_messages(handle.clone(), batches, Origin::Live).await.is_err());

        let stored: Vec<_> = capture.messages(&handle).map(Result::unwrap).collect().await;
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].source_id.as_ref(), "m1");
    }

    #[rstest]
    #[tokio::test]
    async fn waits_out_a_rebuild_over_the_wire() {
        let (mut client, state, _dir) = serve(StoreState::Recovering).await;
        let notices = Arc::new(AtomicUsize::new(0));
        let counter = notices.clone();
        let wait = tokio::spawn(async move {
            client
                .wait_for_sessions(|progress| {
                    // The test's store replays nothing: the daemon reports that it has none to.
                    assert_eq!(progress, (0, 0));
                    counter.fetch_add(1, Ordering::SeqCst);
                })
                .await
        });

        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(!wait.is_finished(), "returned while the daemon was rebuilding");

        state.send_replace(StoreState::Ready);
        tokio::time::timeout(Duration::from_secs(10), wait)
            .await
            .expect("returns once the rebuild ends")
            .unwrap()
            .unwrap();
        // Probed at once, then after 250ms: each probe that finds it rebuilding reports.
        assert!(notices.load(Ordering::SeqCst) >= 2, "each wait reports its progress");
    }

    #[rstest]
    #[case::ready(StoreState::Ready)]
    #[case::failed(StoreState::Unavailable)]
    #[tokio::test]
    async fn returns_at_once_when_not_rebuilding(#[case] state: StoreState) {
        let (mut client, _state, _dir) = serve(state).await;

        let mut announced = false;
        client.wait_for_sessions(|_| announced = true).await.unwrap();

        assert!(!announced);
    }

    /// The status a probe reads without waiting: rebuilding (with its progress) until the
    /// rebuild ends, then not.
    #[rstest]
    #[tokio::test]
    async fn reads_the_rebuild_status_without_waiting() {
        let (mut client, state, _dir) = serve(StoreState::Recovering).await;
        // The test's store replays nothing: the daemon reports that it has none to.
        assert_eq!(client.rebuild_status().await.unwrap(), Some((0, 0)));
        state.send_replace(StoreState::Ready);
        assert_eq!(client.rebuild_status().await.unwrap(), None);
    }
}
