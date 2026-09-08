//! Synchronization primitives.

mod eager_future_cell;
mod striped_mutex;

pub use eager_future_cell::{EagerFuture, EagerFutureCell, MutEagerFutureCell, ResultCell};
pub use striped_mutex::StripedMutex;
