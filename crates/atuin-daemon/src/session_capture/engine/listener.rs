use std::sync::Arc;

use atuin_client::ai_session::HarnessKind;
use atuin_common::harnesstools::session::{Appearance, Session as HSession, WatchError};
use futures::future::BoxFuture;
use futures::stream::{BoxStream, FuturesUnordered};
use futures::{FutureExt, StreamExt};

use super::actor::SessionActor;
use crate::session_capture::Sink;

pub(super) struct HarnessListener<S> {
    kind: HarnessKind,
    sink: Arc<Sink>,
    watch: BoxStream<'static, Result<Appearance<S>, WatchError>>,
}

impl<S> HarnessListener<S>
where
    S: HSession + Send + 'static,
{
    pub(super) fn new(
        kind: HarnessKind,
        sink: Arc<Sink>,
        watch: BoxStream<'static, Result<Appearance<S>, WatchError>>,
    ) -> Self {
        Self { kind, sink, watch }
    }

    pub(super) async fn run(self) {
        let HarnessListener {
            kind,
            sink,
            mut watch,
        } = self;

        let mut actors: FuturesUnordered<BoxFuture<'static, ()>> = FuturesUnordered::new();

        loop {
            tokio::select! {
                appearance = watch.next() => match appearance {
                    Some(Ok(appearance)) => {
                        let actor = SessionActor::new(kind, sink.clone(), appearance);
                        actors.push(actor.run().boxed());
                    }
                    Some(Err(_)) => {}
                    None => break,
                },
                _ = actors.next(), if !actors.is_empty() => {}
            }
        }
    }
}
