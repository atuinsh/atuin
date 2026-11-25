//! The client of the daemon's `desktop_capture.DesktopCapture` service.

use std::num::NonZeroU64;
use std::time::Duration;

use atuin_client::settings::Settings;
use tonic::transport::Channel;
use uuid::Uuid;

use super::{DaemonChannel, DaemonStatusError, FromSettingsError};
use crate::grpc::desktop_capture::pb::desktop_capture_client::DesktopCaptureClient as DesktopCaptureServiceClient;
use crate::grpc::desktop_capture::pb::{self as desktop_capture};
use crate::grpc::request::ClientInterceptor;
use crate::service::ActiveService;
use crate::service::address::{ConnectError, DaemonAddress};

/// Client for the daemon's `desktop_capture.DesktopCapture` service, returning the generated
/// protobuf messages.
#[derive(Clone)]
pub struct DesktopCaptureClient {
    client: DesktopCaptureServiceClient<DaemonChannel>,
}

/// The client takes replies of any size, because a listing of a large store can pass tonic's
/// default 4 MiB. The daemon is local and trusted, and its server encodes any size by default.
impl From<Channel> for DesktopCaptureClient {
    fn from(channel: Channel) -> Self {
        Self {
            client: DesktopCaptureServiceClient::with_interceptor(channel, ClientInterceptor)
                .max_decoding_message_size(usize::MAX),
        }
    }
}

impl DesktopCaptureClient {
    pub async fn new(address: impl Into<DaemonAddress>) -> Result<Self, ConnectError> {
        Ok(Self::from(address.into().channel().await?))
    }

    pub async fn from_settings(settings: &Settings) -> Result<Self, FromSettingsError> {
        Ok(Self::new(ActiveService::locate(&settings.daemon)?).await?)
    }

    pub async fn status(&mut self) -> Result<desktop_capture::StatusReply, DaemonStatusError> {
        Ok(self.client.status(desktop_capture::StatusRequest {}).await?.into_inner())
    }

    /// Stops capture until [`Self::start`], across daemon restarts.
    pub async fn stop(&mut self) -> Result<(), DaemonStatusError> {
        self.client.stop(desktop_capture::StopRequest {}).await?;
        Ok(())
    }

    /// Undoes [`Self::stop`], and starts a helper at once if no helper is running. The reply waits
    /// up to `outcome_wait` for the outcome of that helper, which the status then shows.
    pub async fn start(&mut self, outcome_wait: Duration) -> Result<(), DaemonStatusError> {
        let outcome_wait_ms = u32::try_from(outcome_wait.as_millis()).unwrap_or(u32::MAX);
        self.client.start(desktop_capture::StartRequest { outcome_wait_ms }).await?;
        Ok(())
    }

    /// Tells the daemon that a desktop session started.
    pub async fn session_started(&mut self) -> Result<(), DaemonStatusError> {
        self.client.session_started(desktop_capture::SessionStartedRequest {}).await?;
        Ok(())
    }

    pub async fn recent_activity(
        &mut self,
        request: desktop_capture::RecentActivityRequest,
    ) -> Result<desktop_capture::RecentActivityReply, DaemonStatusError> {
        Ok(self.client.recent_activity(request).await?.into_inner())
    }

    pub async fn list_segments(
        &mut self,
        request: desktop_capture::ListSegmentsRequest,
    ) -> Result<desktop_capture::ListSegmentsReply, DaemonStatusError> {
        Ok(self.client.list_segments(request).await?.into_inner())
    }

    /// Deletes the segment that `id` names: the id of the segment or of one of its records.
    pub async fn delete_id(
        &mut self,
        id: Uuid,
    ) -> Result<desktop_capture::DeleteReply, DaemonStatusError> {
        self.delete(desktop_capture::delete_request::Selection::Id(id.to_string())).await
    }

    /// Deletes the segments that `filter` selects.
    pub async fn delete_matching(
        &mut self,
        filter: desktop_capture::DeleteFilter,
    ) -> Result<desktop_capture::DeleteReply, DaemonStatusError> {
        self.delete(desktop_capture::delete_request::Selection::Filter(filter)).await
    }

    /// Deletes every segment and every damaged record.
    pub async fn delete_everything(
        &mut self,
    ) -> Result<desktop_capture::DeleteReply, DaemonStatusError> {
        let everything = desktop_capture::DeleteEverything {};
        self.delete(desktop_capture::delete_request::Selection::Everything(everything)).await
    }

    async fn delete(
        &mut self,
        selection: desktop_capture::delete_request::Selection,
    ) -> Result<desktop_capture::DeleteReply, DaemonStatusError> {
        let request = desktop_capture::DeleteRequest {
            selection: Some(selection),
        };
        Ok(self.client.delete(request).await?.into_inner())
    }

    /// Answers which segment `id` names: the id of the segment or of one of its records.
    pub async fn lookup(
        &mut self,
        id: Uuid,
    ) -> Result<desktop_capture::LookupReply, DaemonStatusError> {
        let request = desktop_capture::LookupRequest { id: id.to_string() };
        Ok(self.client.lookup(request).await?.into_inner())
    }

    /// Reads the text of the snapshot that `id` names, or of the latest snapshot of the segment
    /// that `id` names.
    ///
    /// A text longer than `max_bytes` keeps its beginning and its end. Without `max_bytes`, the
    /// read returns the whole text.
    pub async fn read_by_id(
        &mut self,
        id: Uuid,
        max_bytes: Option<NonZeroU64>,
    ) -> Result<desktop_capture::ReadSegmentReply, DaemonStatusError> {
        let request = desktop_capture::ReadSegmentRequest {
            id: id.to_string(),
            which: desktop_capture::Which::Unspecified.into(),
            record_id: String::new(),
            max_bytes: max_bytes.map_or(0, NonZeroU64::get),
        };
        Ok(self.client.read_segment(request).await?.into_inner())
    }

    /// Reads the text of the `choice` snapshot of the segment that `id` names: the segment, or
    /// one of its snapshots.
    ///
    /// A text longer than `max_bytes` keeps its beginning and its end. Without `max_bytes`, the
    /// read returns the whole text.
    pub async fn read_snapshot(
        &mut self,
        id: Uuid,
        choice: SnapshotChoice,
        max_bytes: Option<NonZeroU64>,
    ) -> Result<desktop_capture::ReadSegmentReply, DaemonStatusError> {
        let (which, record_id) = match choice {
            SnapshotChoice::First => (desktop_capture::Which::First, String::new()),
            SnapshotChoice::Latest => (desktop_capture::Which::Latest, String::new()),
            SnapshotChoice::Record(record) => (desktop_capture::Which::Record, record.to_string()),
        };
        let request = desktop_capture::ReadSegmentRequest {
            id: id.to_string(),
            which: which.into(),
            record_id,
            max_bytes: max_bytes.map_or(0, NonZeroU64::get),
        };
        Ok(self.client.read_segment(request).await?.into_inner())
    }

    /// Reads the texts of `snapshots` from the segment that `id` names, in the order of the
    /// segment's snapshots.
    ///
    /// The snapshots come from a listing of the segment, and `id` names the segment or one of
    /// its snapshots. The reply ends before the first text that would take the texts past
    /// `budget.total`, but it always holds the first text.
    /// [`ReadSnapshotsReply::left_out`](desktop_capture::ReadSnapshotsReply::left_out) counts
    /// the snapshots that the reply leaves out.
    pub async fn read_listed_snapshots<'s>(
        &mut self,
        id: Uuid,
        snapshots: impl IntoIterator<Item = &'s desktop_capture::Snapshot>,
        budget: SnapshotsBudget,
    ) -> Result<desktop_capture::ReadSnapshotsReply, DaemonStatusError> {
        let request = desktop_capture::ReadSnapshotsRequest {
            id: id.to_string(),
            record_ids: snapshots.into_iter().map(|snapshot| snapshot.record_id.clone()).collect(),
            max_bytes: budget.per_text.map_or(0, NonZeroU64::get),
            max_total_bytes: budget.total.get(),
        };
        Ok(self.client.read_snapshots(request).await?.into_inner())
    }

    /// Lists the snapshots of the segment that `id` names: the segment, or one of its snapshots.
    pub async fn list_snapshots(
        &mut self,
        id: Uuid,
    ) -> Result<desktop_capture::ListSnapshotsReply, DaemonStatusError> {
        let request = desktop_capture::ListSnapshotsRequest { id: id.to_string() };
        Ok(self.client.list_snapshots(request).await?.into_inner())
    }

    /// Counts the segments that `tallied` selects, per application.
    pub async fn tally_segments(
        &mut self,
        tallied: Tallied,
    ) -> Result<desktop_capture::TallySegmentsReply, DaemonStatusError> {
        let Tallied {
            from_ms,
            to_ms,
            app,
        } = tallied;
        let request = desktop_capture::TallySegmentsRequest {
            from_ms,
            to_ms: to_ms.unwrap_or(0),
            app,
        };
        Ok(self.client.tally_segments(request).await?.into_inner())
    }

    pub async fn search(
        &mut self,
        request: desktop_capture::SearchRequest,
    ) -> Result<desktop_capture::SearchReply, DaemonStatusError> {
        Ok(self.client.search(request).await?.into_inner())
    }
}

/// The longest snippet of a search hit that a client shows, in characters.
///
/// The daemon cuts snippets to 200 characters. This cap is higher, so it shortens only the
/// snippets of a daemon that cuts them at a longer length.
pub const MAX_SNIPPET_CHARS: usize = 300;

/// The segments that [`DesktopCaptureClient::tally_segments`] counts: those that overlap the
/// range from `from_ms` to `to_ms`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tallied {
    pub from_ms: i64,
    /// `None` reaches to now.
    pub to_ms: Option<i64>,
    /// Only the segments of this application, compared without regard to case.
    pub app: Option<String>,
}

/// How much text [`DesktopCaptureClient::read_listed_snapshots`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotsBudget {
    /// The longest text of each snapshot, in bytes. A longer text keeps its beginning and its
    /// end. `None` limits each text only by `total`.
    pub per_text: Option<NonZeroU64>,
    /// The most bytes that the texts hold together, which also limits each text. The daemon
    /// caps this at 16 MiB.
    pub total: NonZeroU64,
}

/// Which snapshot of a segment [`DesktopCaptureClient::read_snapshot`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotChoice {
    /// How the window looked when the segment began.
    First,
    /// How the window looked last.
    Latest,
    /// The snapshot that this record id names, as a search or a listing of snapshots gives it.
    Record(Uuid),
}
