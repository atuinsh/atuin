use std::num::NonZeroU32;

use prost::Message;

use super::{CapturedThreadMessage, History, InsertHistoryRequest, InsertThreadMessagesRequest};

/// The items of one `InsertHistory` or `InsertThreadMessages`: at most [`Self::MAX_ITEMS`],
/// encoding to at most 1 MiB unless the batch holds one. Only [`Self::batches`] and [`Self::one`]
/// build one and [`Self::halves`] splits one, so no request past those limits goes up.
#[derive(Debug, Clone)]
pub struct Batch<M>(Vec<M>);

pub type ThreadBatch = Batch<CapturedThreadMessage>;
pub type HistoryBatch = Batch<History>;

impl<M: Message> Batch<M> {
    /// The most items one request takes.
    pub const MAX_ITEMS: NonZeroU32 = NonZeroU32::new(500).unwrap();
    /// The most a batch of more than one item encodes to: its request goes up inside the default
    /// 30s `network_timeout` at 0.28 Mbit/s, and a lone item, which conversion caps at 1 MiB too,
    /// stays far under the 4 MiB a server may cap a request body at.
    pub(super) const MAX_BYTES: usize = 1 << 20;

    /// `items` in order, cut into batches.
    #[must_use]
    pub fn batches(items: Vec<M>) -> Vec<Self> {
        let max_items = Self::MAX_ITEMS.get() as usize;
        let mut batches: Vec<Self> = Vec::new();
        let mut batch_bytes = 0;
        for item in items {
            let len = Self::framed_len(&item);
            match batches.last_mut() {
                Some(Self(batch))
                    if batch.len() < max_items && batch_bytes + len <= Self::MAX_BYTES =>
                {
                    batch_bytes += len;
                    batch.push(item);
                }
                _ => {
                    batch_bytes = len;
                    batches.push(Self(vec![item]));
                }
            }
        }
        batches
    }

    #[must_use]
    pub fn one(item: M) -> Self {
        Self(vec![item])
    }

    /// The batch in two, the earlier items first, or the batch back whole when it holds one item.
    /// Each half of a batch is one too.
    pub fn halves(mut self) -> Result<(Self, Self), Self> {
        if self.0.len() < 2 {
            return Err(self);
        }
        let back = self.0.split_off(self.0.len() / 2);
        Ok((self, Self(back)))
    }

    #[must_use]
    pub fn items(&self) -> &[M] {
        &self.0
    }

    /// What `item` adds to the request holding it, whose only field is the repeated field 1.
    fn framed_len(item: &M) -> usize {
        let len = item.encoded_len();
        // The tag of field 1 takes a byte.
        1 + prost::length_delimiter_len(len) + len
    }
}

impl From<ThreadBatch> for InsertThreadMessagesRequest {
    fn from(Batch(messages): ThreadBatch) -> Self {
        Self { messages }
    }
}

impl From<HistoryBatch> for InsertHistoryRequest {
    fn from(Batch(entries): HistoryBatch) -> Self {
        Self { entries }
    }
}
