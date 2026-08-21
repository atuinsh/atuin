//! Synchronization primitives.

mod eager_future_cell;

pub use eager_future_cell::{EagerFuture, EagerFutureCell, MutEagerFutureCell, ResultCell};
