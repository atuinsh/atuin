//! Callsites represent the source locations from which spans or events
//! originate.
use crate::stdlib::{
    fmt,
    hash::{Hash, Hasher},
    sync::Mutex,
    vec::Vec,
};
use crate::{
    dispatcher::{self, Dispatch},
    metadata::{LevelFilter, Metadata},
    subscriber::Interest,
};

lazy_static! {
    static ref REGISTRY: Mutex<Registry> = Mutex::new(Registry {
        callsites: Vec::new(),
        dispatchers: Vec::new(),
    });
}

struct Registry {
    callsites: Vec<&'static dyn Callsite>,
    dispatchers: Vec<dispatcher::Registrar>,
}

impl Registry {
    fn rebuild_callsite_interest(&self, callsite: &'static dyn Callsite) {
        let meta = callsite.metadata();

        // Iterate over the subscribers in the registry, and — if they are
        // active — register the callsite with them.
        let mut interests = self
            .dispatchers
            .iter()
            .filter_map(|registrar| registrar.try_register(meta));

        // Use the first subscriber's `Interest` as the base value.
        let interest = if let Some(interest) = interests.next() {
            // Combine all remaining `Interest`s.
            interests.fold(interest, Interest::and)
        } else {
            // If nobody was interested in this thing, just return `never`.
            Interest::never()
        };

        callsite.set_interest(interest)
    }

    fn rebuild_interest(&mut self) {
        let mut max_level = LevelFilter::OFF;
        self.dispatchers.retain(|registrar| {
            if let Some(dispatch) = registrar.upgrade() {
                // If the subscriber did not provide a max level hint, assume
                // that it may enable every level.
                let level_hint = dispatch.max_level_hint().unwrap_or(LevelFilter::TRACE);
                if level_hint > max_level {
                    max_level = level_hint;
                }
                true
            } else {
                false
            }
        });

        self.callsites.iter().for_each(|&callsite| {
            self.rebuild_callsite_interest(callsite);
        });
        LevelFilter::set_max(max_level);
    }
}

/// Trait implemented by callsites.
///
/// These functions are only intended to be called by the callsite registry, which
/// correctly handles determining the common interest between all subscribers.
pub trait Callsite: Sync {
    /// Sets the [`Interest`] for this callsite.
    ///
    /// [`Interest`]: ../subscriber/struct.Interest.html
    fn set_interest(&self, interest: Interest);

    /// Returns the [metadata] associated with the callsite.
    ///
    /// [metadata]: ../metadata/struct.Metadata.html
    fn metadata(&self) -> &Metadata<'_>;
}

/// Uniquely identifies a [`Callsite`]
///
/// Two `Identifier`s are equal if they both refer to the same callsite.
///
/// [`Callsite`]: ../callsite/trait.Callsite.html
#[derive(Clone)]
pub struct Identifier(
    /// **Warning**: The fields on this type are currently `pub` because it must
    /// be able to be constructed statically by macros. However, when `const
    /// fn`s are available on stable Rust, this will no longer be necessary.
    /// Thus, these fields are *not* considered stable public API, and they may
    /// change warning. Do not rely on any fields on `Identifier`. When
    /// constructing new `Identifier`s, use the `identify_callsite!` macro or
    /// the `Callsite::id` function instead.
    // TODO: When `Callsite::id` is a const fn, this need no longer be `pub`.
    #[doc(hidden)]
    pub &'static dyn Callsite,
);

/// Clear and reregister interest on every [`Callsite`]
///
/// This function is intended for runtime reconfiguration of filters on traces
/// when the filter recalculation is much less frequent than trace events are.
/// The alternative is to have the [`Subscriber`] that supports runtime
/// reconfiguration of filters always return [`Interest::sometimes()`] so that
/// [`enabled`] is evaluated for every event.
///
/// This function will also re-compute the global maximum level as determined by
/// the [`max_level_hint`] method. If a [`Subscriber`]
/// implementation changes the value returned by its `max_level_hint`
/// implementation at runtime, then it **must** call this function after that
/// value changes, in order for the change to be reflected.
///
/// [`max_level_hint`]: ../subscriber/trait.Subscriber.html#method.max_level_hint
/// [`Callsite`]: ../callsite/trait.Callsite.html
/// [`enabled`]: ../subscriber/trait.Subscriber.html#tymethod.enabled
/// [`Interest::sometimes()`]: ../subscriber/struct.Interest.html#method.sometimes
/// [`Subscriber`]: ../subscriber/trait.Subscriber.html
pub fn rebuild_interest_cache() {
    let mut registry = REGISTRY.lock().unwrap();
    registry.rebuild_interest();
}

/// Register a new `Callsite` with the global registry.
///
/// This should be called once per callsite after the callsite has been
/// constructed.
pub fn register(callsite: &'static dyn Callsite) {
    let mut registry = REGISTRY.lock().unwrap();
    registry.rebuild_callsite_interest(callsite);
    registry.callsites.push(callsite);
}

pub(crate) fn register_dispatch(dispatch: &Dispatch) {
    let mut registry = REGISTRY.lock().unwrap();
    registry.dispatchers.push(dispatch.registrar());
    registry.rebuild_interest();
}

// ===== impl Identifier =====

impl PartialEq for Identifier {
    fn eq(&self, other: &Identifier) -> bool {
        self.0 as *const _ as *const () == other.0 as *const _ as *const ()
    }
}

impl Eq for Identifier {}

impl fmt::Debug for Identifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Identifier({:p})", self.0)
    }
}

impl Hash for Identifier {
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        (self.0 as *const dyn Callsite).hash(state)
    }
}
