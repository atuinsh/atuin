//! Routes the records of one helper run to the store and the index.

use std::time::SystemTime;

use atuin_desktop_capture::Record;
use tracing::{debug, warn};

use super::opened::Opened;
use crate::desktop_capture::stop_file::Requested;
use crate::desktop_capture::store::{RunState, Written};

/// The records of one helper run on their way to the store.
#[derive(Debug, Default)]
pub struct RunWriter {
    records: RunState,
    /// The records that a stop kept out of the store.
    dropped: u64,
}

impl Drop for RunWriter {
    fn drop(&mut self) {
        if self.dropped > 0 {
            debug!("dropped {} desktop capture records while capture was stopped", self.dropped);
        }
    }
}

impl RunWriter {
    /// Writes `record` to the store, and its new content to the index.
    ///
    /// While capture is stopped, only the rest of a segment that was open before the stop is
    /// written: the snapshots and the end that the helper sends as it pauses.
    pub async fn write(&mut self, opened: &Opened, requested: Requested, record: Record) {
        if requested == Requested::Stopped && !self.continues_open_segment(&record) {
            self.dropped += 1;
            return;
        }
        // The first failure of a segment's start is logged, and each later record of the
        // segment tries the start again.
        let unstarted = match &record {
            Record::Snapshot(snapshot) if self.records.is_unstarted(snapshot.segment) => {
                Some(snapshot.segment)
            }
            _ => None,
        };
        // The store ends a dialog or a notification in the commit that writes it, so an
        // eviction can remove the segment at once. The guard keeps such an eviction out until
        // the index holds the segment's content, so that the index can't add rows of a segment
        // that the store no longer holds.
        let following = match (&record, opened.index()) {
            (Record::Dialog(_) | Record::Notification(_), Some(index)) => {
                Some(index.follow_store().await)
            }
            _ => None,
        };
        let written = opened.store().write(&mut self.records, record).await;
        let retried_start_failed = unstarted.is_some_and(|id| self.records.is_unstarted(id));
        match written {
            Ok(Written { item: Some(item) }) => {
                if let Some(index) = opened.index()
                    && let Err(err) = index.add(std::iter::once(item)).await
                {
                    warn!(?err, "could not index the desktop capture content");
                }
            }
            Ok(Written { item: None }) => {}
            Err(err) if retried_start_failed => {
                debug!(?err, "could not store the start of a desktop capture segment again");
            }
            Err(err) => warn!(?err, "could not store a desktop capture record"),
        }
        drop(following);
    }

    /// Ends the background segments that the run has not written to for a while.
    pub async fn end_idle_background(&mut self, opened: &Opened) {
        let ended = opened.store().end_idle_background(&mut self.records, SystemTime::now()).await;
        if let Err(err) = ended {
            warn!(?err, "could not end the idle desktop capture background segments");
        }
    }

    /// Ends every background segment of the run now, because the run stopped reading the
    /// background windows. The next read of a window starts a new segment.
    pub async fn end_background(&mut self, opened: &Opened) {
        let ended = opened.store().end_background(&mut self.records, SystemTime::now()).await;
        if let Err(err) = ended {
            warn!(?err, "could not end the desktop capture background segments");
        }
    }

    /// Whether `record` continues a segment that the run started before the stop.
    ///
    /// A snapshot continues a segment whose start this run stored. An end also continues a
    /// segment whose start the store could not write, so that the run forgets the segment's
    /// start. The end of such a segment doesn't store anything.
    fn continues_open_segment(&self, record: &Record) -> bool {
        match record {
            Record::Snapshot(snapshot) => self.records.is_open(snapshot.segment),
            Record::SegmentEnded { segment, .. } => self.records.has_segment(*segment),
            Record::SegmentStarted(_)
            | Record::Dialog(_)
            | Record::Notification(_)
            | Record::Background(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use atuin_desktop_capture::SegmentId;
    use rstest::rstest;

    use super::*;
    use crate::desktop_capture::supervisor::opened::Storage;
    use crate::desktop_capture::testing::records::{
        dialog, ended, everything, lines, snapshot, window,
    };
    use crate::desktop_capture::testing::{HOST, Home, home};

    /// The write of a dialog waits while an eviction is between the store and the index, so
    /// that the eviction can't remove the dialog between the dialog's write and its indexing.
    #[rstest]
    #[tokio::test]
    async fn a_dialog_waits_for_the_index_to_follow_the_store(home: Home) {
        let storage = Storage::new(&home.settings(&["gedit"]), HOST);
        let opened = storage.get_or_open().await.expect("open");
        let index = opened.index().expect("an index");
        let mut writer = RunWriter::default();
        let following = index.follow_store().await;

        let record = dialog("gedit", "Save", SystemTime::now(), window("zebra"));
        let mut write = std::pin::pin!(writer.write(opened, Requested::Running, record));
        let timeout = std::time::Duration::from_millis(200);
        let waited = tokio::time::timeout(timeout, write.as_mut()).await;
        assert!(waited.is_err(), "the dialog was written while the index was following");
        assert!(opened.store().segments(everything()).await.expect("segments").is_empty());
        drop(following);

        write.await;
        assert_eq!(opened.store().segments(everything()).await.expect("segments").len(), 1);
        opened.close().await;
    }

    /// While capture is stopped, the end of a segment whose start the store could not write
    /// lets the run forget the segment, and a snapshot of the segment doesn't store anything.
    #[rstest]
    #[tokio::test]
    async fn a_stop_lets_an_unstarted_segment_end(home: Home) {
        let storage = Storage::new(&home.settings(&["gedit"]), HOST);
        let opened = storage.get_or_open().await.expect("open");
        let mut writer = RunWriter::default();
        writer.records.add_unstarted(SegmentId(1), SystemTime::now());

        let stopped = Requested::Stopped;
        writer.write(opened, stopped, snapshot(1, SystemTime::now(), lines(1))).await;
        writer.write(opened, stopped, ended(1, SystemTime::now())).await;

        assert!(!writer.records.has_segment(SegmentId(1)));
        assert!(opened.store().segments(everything()).await.expect("segments").is_empty());
        opened.close().await;
    }
}
