//! What the daemon would tell the user, streamed to the client whose request it answers.

pub mod pb;

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::pin::Pin;
use std::sync::Arc;

use atuin_common::userio::log::{Message, RequestId, Sink, Stderr};
use futures::StreamExt as _;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio_stream::Stream;
use tonic::{Request, Response, Status, Streaming};

use crate::grpc::request;
use crate::grpc::userio::pb::user_io_server::UserIo as GrpcService;
use crate::grpc::userio::pb::{ListenLogsReply, ListenLogsRequest};

/// The clients listening for messages, by the request they listen for.
type Listeners = Arc<Mutex<HashMap<RequestId, mpsc::Sender<Message>>>>;

/// The daemon's sink, which has no terminal: it sends each message to the client listening for the
/// request it answers, to show to their user.
#[derive(Clone)]
pub struct Service {
    listeners: Listeners,
    /// Where a message no client listens for goes: one from the daemon's own work, one for a client
    /// that left, or one more than its client could take.
    fallback: Arc<dyn Sink>,
}

impl Service {
    /// How many messages a client may fall behind by before the rest go to the fallback.
    const BACKLOG: usize = 64;

    fn new(fallback: Arc<dyn Sink>) -> Self {
        Self {
            listeners: Listeners::default(),
            fallback,
        }
    }
}

impl Default for Service {
    /// The service whose fallback is stderr, which launchd and systemd keep and a daemon a client
    /// spawned sends to `/dev/null`.
    fn default() -> Self {
        Self::new(Arc::new(Stderr::default()))
    }
}

impl Sink for Service {
    fn show(&self, message: Message) {
        let listener =
            message.request.and_then(|request| self.listeners.lock().get(&request).cloned());
        let Some(listener) = listener else {
            self.fallback.show(message);
            return;
        };
        if let Err(TrySendError::Full(message) | TrySendError::Closed(message)) =
            listener.try_send(message)
        {
            self.fallback.show(message);
        }
    }
}

/// A client listening for a request's messages, until dropped.
struct Listener {
    listeners: Listeners,
    request: RequestId,
}

impl Listener {
    fn register(
        listeners: &Listeners,
        request: RequestId,
        messages: mpsc::Sender<Message>,
    ) -> Result<Self, Status> {
        match listeners.lock().entry(request) {
            Entry::Occupied(_) => {
                return Err(Status::already_exists(format!(
                    "a client already listens for request {request}"
                )));
            }
            Entry::Vacant(entry) => entry.insert(messages),
        };
        Ok(Self {
            listeners: Arc::clone(listeners),
            request,
        })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.listeners.lock().remove(&self.request);
    }
}

#[tonic::async_trait]
impl GrpcService for Service {
    type ListenLogsStream = Pin<Box<dyn Stream<Item = Result<ListenLogsReply, Status>> + Send>>;

    async fn listen_logs(
        &self,
        request: Request<Streaming<ListenLogsRequest>>,
    ) -> Result<Response<Self::ListenLogsStream>, Status> {
        let Some(id) = request::of(request.metadata().as_ref()) else {
            return Err(Status::invalid_argument("the call names no request to listen for"));
        };
        let (sender, mut messages) = mpsc::channel(Self::BACKLOG);
        let listener = Listener::register(&self.listeners, id, sender)?;
        let mut requests = request.into_inner();

        let replies = async_stream::stream! {
            loop {
                tokio::select! {
                    biased;
                    Some(message) = messages.recv() => {
                        yield Ok(ListenLogsReply { message: Some(message.into()) });
                    }
                    request = requests.next() => match request {
                        Some(Ok(ListenLogsRequest {})) => {}
                        Some(Err(_)) | None => break,
                    },
                }
            }
            // The client closed its side: what is shown for its request from now on goes to the
            // fallback, and what was shown until now is sent before the stream ends.
            drop(listener);
            while let Ok(message) = messages.try_recv() {
                yield Ok(ListenLogsReply { message: Some(message.into()) });
            }
        };

        Ok(Response::new(Box::pin(replies)))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::sync::Arc;

    use atuin_common::userio::log::{Held, Message, RequestId, SINK, Severity, Sink};
    use futures::StreamExt as _;
    use rstest::rstest;
    use tokio::net::UnixListener;
    use tokio::sync::oneshot;
    use tokio_stream::wrappers::UnixListenerStream;
    use tonic::transport::{Channel, Server};

    use super::Service;
    use crate::client::LogListener;
    use crate::grpc::request::{self, ClientInterceptor};
    use crate::grpc::userio::pb::user_io_client::UserIoClient;
    use crate::grpc::userio::pb::user_io_server::UserIoServer;
    use crate::grpc::userio::pb::{ListenLogsReply, ListenLogsRequest};
    use crate::service::address::DaemonAddress;

    /// A daemon serving only `service`, and a channel to it.
    async fn serve(service: &Service) -> (tempfile::TempDir, Channel) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.sock");
        let incoming = UnixListenerStream::new(UnixListener::bind(&path).unwrap());
        tokio::spawn(
            Server::builder()
                .add_service(UserIoServer::new(service.clone()))
                .serve_with_incoming(incoming),
        );
        let channel = DaemonAddress::from(path).channel().await.unwrap();
        (dir, channel)
    }

    fn message(text: &str, request: Option<RequestId>) -> Message {
        Message {
            severity: Severity::Warn,
            text: text.to_owned(),
            request,
        }
    }

    // One case: the client shows on the process's `SINK`, which cases run in parallel would share.
    #[rstest]
    #[tokio::test]
    async fn a_client_shows_every_message_for_its_request_shown_before_it_finished() {
        let shown = Held::default();
        let _shown = SINK.push(shown.clone());
        let service = Service::default();
        let (_dir, channel) = serve(&service).await;
        let request = RequestId::generate();

        let logs = request::scope(request, LogListener::listen(channel)).await.unwrap();
        let sent: Vec<_> = (0..Service::BACKLOG)
            .map(|i| message(&format!("message {i}"), Some(request)))
            .collect();
        for message in &sent {
            service.show(message.clone());
        }
        logs.finish().await;

        let texts =
            |messages: Vec<Message>| messages.into_iter().map(|m| m.text).collect::<Vec<_>>();
        assert_eq!(texts(shown.take()), texts(sent));
    }

    #[rstest]
    #[tokio::test]
    async fn each_message_reaches_only_the_client_of_its_request() {
        /// Listens for `request`'s messages until `close` is dropped.
        async fn listen(
            channel: Channel,
            request: RequestId,
        ) -> (oneshot::Sender<()>, tonic::Streaming<ListenLogsReply>) {
            let (close, closed) = oneshot::channel::<()>();
            let requests =
                futures::stream::once(closed).filter_map(|_| async { None::<ListenLogsRequest> });
            let mut client = UserIoClient::with_interceptor(channel, ClientInterceptor);
            let replies = request::scope(request, client.listen_logs(requests)).await.unwrap();
            (close, replies.into_inner())
        }

        /// The texts of what the daemon sent, once `close` ends the stream.
        async fn received(
            (close, mut replies): (oneshot::Sender<()>, tonic::Streaming<ListenLogsReply>),
        ) -> Vec<String> {
            drop(close);
            let mut texts = Vec::new();
            while let Some(reply) = replies.message().await.unwrap() {
                texts.push(reply.message.unwrap().text);
            }
            texts
        }

        let fallback = Held::default();
        let service = Service::new(Arc::new(fallback.clone()));
        let (_dir, channel) = serve(&service).await;
        let (a, b) = (RequestId::generate(), RequestId::generate());

        let listening_a = listen(channel.clone(), a).await;
        let listening_b = listen(channel, b).await;
        service.show(message("for a", Some(a)));
        service.show(message("for b", Some(b)));
        service.show(message("for nobody", Some(RequestId::generate())));
        service.show(message("the daemon's own", None));

        assert_eq!(received(listening_a).await, ["for a"]);
        assert_eq!(received(listening_b).await, ["for b"]);
        let fallen_back: Vec<_> = fallback.take().into_iter().map(|m| m.text).collect();
        assert_eq!(fallen_back, ["for nobody", "the daemon's own"]);
    }
}
