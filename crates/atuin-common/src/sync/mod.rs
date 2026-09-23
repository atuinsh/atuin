//! Synchronization primitives.

mod eager_future_cell;
mod sharded_mutex;
mod wakeup;

pub use eager_future_cell::{EagerFuture, EagerFutureCell, MutEagerFutureCell, ResultCell};
pub use sharded_mutex::{AsyncShardedMutex, ShardedMutex};
pub use wakeup::Wakeup;
