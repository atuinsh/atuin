//! Collects and records trace data.
pub use tracing_core::subscriber::*;

#[cfg(feature = "std")]
#[cfg_attr(docsrs, doc(cfg(feature = "std")))]
pub use tracing_core::dispatcher::DefaultGuard;

/// Sets this subscriber as the default for the duration of a closure.
///
/// The default subscriber is used when creating a new [`Span`] or
/// [`Event`], _if no span is currently executing_. If a span is currently
/// executing, new spans or events are dispatched to the subscriber that
/// tagged that span, instead.
///
/// [`Span`]: ../span/struct.Span.html
/// [`Subscriber`]: ../subscriber/trait.Subscriber.html
/// [`Event`]: :../event/struct.Event.html
#[cfg(feature = "std")]
#[cfg_attr(docsrs, doc(cfg(feature = "std")))]
pub fn with_default<T, S>(subscriber: S, f: impl FnOnce() -> T) -> T
where
    S: Subscriber + Send + Sync + 'static,
{
    crate::dispatcher::with_default(&crate::Dispatch::new(subscriber), f)
}

/// Sets this subscriber as the global default for the duration of the entire program.
/// Will be used as a fallback if no thread-local subscriber has been set in a thread (using `with_default`.)
///
/// Can only be set once; subsequent attempts to set the global default will fail.
/// Returns whether the initialization was successful.
///
/// Note: Libraries should *NOT* call `set_global_default()`! That will cause conflicts when
/// executables try to set them later.
///
/// [span]: ../span/index.html
/// [`Subscriber`]: ../subscriber/trait.Subscriber.html
/// [`Event`]: ../event/struct.Event.html
pub fn set_global_default<S>(subscriber: S) -> Result<(), SetGlobalDefaultError>
where
    S: Subscriber + Send + Sync + 'static,
{
    crate::dispatcher::set_global_default(crate::Dispatch::new(subscriber))
}

/// Sets the subscriber as the default for the duration of the lifetime of the
/// returned [`DefaultGuard`]
///
/// The default subscriber is used when creating a new [`Span`] or
/// [`Event`], _if no span is currently executing_. If a span is currently
/// executing, new spans or events are dispatched to the subscriber that
/// tagged that span, instead.
///
/// [`Span`]: ../span/struct.Span.html
/// [`Subscriber`]: ../subscriber/trait.Subscriber.html
/// [`Event`]: :../event/struct.Event.html
/// [`DefaultGuard`]: ../dispatcher/struct.DefaultGuard.html
#[cfg(feature = "std")]
#[cfg_attr(docsrs, doc(cfg(feature = "std")))]
#[must_use = "Dropping the guard unregisters the subscriber."]
pub fn set_default<S>(subscriber: S) -> DefaultGuard
where
    S: Subscriber + Send + Sync + 'static,
{
    crate::dispatcher::set_default(&crate::Dispatch::new(subscriber))
}

pub use tracing_core::dispatcher::SetGlobalDefaultError;
