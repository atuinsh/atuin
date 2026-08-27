//! Control service implementation.
//!
//! This gRPC service allows external processes (like CLI commands) to inject
//! events into the daemon's event bus.

use atuin_client::history::HistoryId;
use tonic::{Request, Response, Status};
use tracing::{Level, info, instrument};

use super::control_server::{Control, ControlServer};
use super::send_event_request::Event;
use super::{SendEventRequest, SendEventResponse};
use crate::daemon::DaemonHandle;
use crate::events::DaemonEvent;

/// The Control gRPC service.
///
/// This service is used by external processes to inject events into the daemon.
/// It's not a component - it's part of the daemon's core infrastructure.
pub struct ControlService {
    handle: DaemonHandle,
}

impl ControlService {
    /// Create a new control service with the given daemon handle.
    #[must_use]
    pub fn new(handle: DaemonHandle) -> Self {
        Self { handle }
    }

    /// Get a tonic server for this service.
    #[must_use]
    pub fn into_server(self) -> ControlServer<Self> {
        ControlServer::new(self)
    }
}

#[tonic::async_trait]
impl Control for ControlService {
    #[instrument(skip_all, level = Level::INFO, name = "control_send_event")]
    async fn send_event(
        &self,
        request: Request<SendEventRequest>,
    ) -> Result<Response<SendEventResponse>, Status> {
        let req = request.into_inner();

        let event = req.event.ok_or_else(|| Status::invalid_argument("event is required"))?;

        let daemon_event = proto_event_to_daemon_event(event);

        info!(?daemon_event, "received control event");
        self.handle.emit(daemon_event);

        Ok(Response::new(SendEventResponse {}))
    }
}

/// Convert a proto event to a daemon event.
fn proto_event_to_daemon_event(event: Event) -> DaemonEvent {
    match event {
        Event::HistoryPruned(_) => DaemonEvent::HistoryPruned,
        Event::HistoryRebuilt(_) => DaemonEvent::HistoryRebuilt,
        Event::HistoryDeleted(e) => DaemonEvent::HistoryDeleted {
            ids: e.ids.into_iter().map(HistoryId).collect(),
        },
        Event::ForceSync(_) => DaemonEvent::ForceSync,
        Event::SettingsReloaded(_) => DaemonEvent::SettingsReloaded,
        Event::Shutdown(_) => DaemonEvent::ShutdownRequested,
    }
}
