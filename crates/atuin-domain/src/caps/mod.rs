//! Capability system used by atuin.
//!
//! # Context
//!
//! A node advertises capabilities about itself and, if it is a client, can read the server's.
//!
//! Atuin's client and server versions are not necessarily always compatible. There are features
//! that clients may support, but outdated servers will not.
//!
//! The capability system is designed to help us bridge the gap between the two.
//!
//! # Design
//!
//! - Each capability has a unique `CRI` (capability resource identifier), eg.
//!   `sh.atuin.server/capabilities`.
//! - Each capability has arbitrary associated data, for example `{ "version": 1 }`.
//!
//! The client passes a header with each request it makes, `x-atuin-capabilities-known: <hash>`
//! which communicates to the server what capabilities the client is aware of. If the server's
//! capability hash does not match that of what the client believes, the server rejects the request
//! with a 412, after which the client polls `/api/v0/capabilities` to get the new capability list
//! as well as the new hash of the capability list.
//!
//! The client then passes this updated hash back to the server and all is well.
//!
//! The server capabilities are sent with every response as part of `x-atuin-capabilities-available`
//! in order to eagerly communicate to the client that the capability set needs to be updated
//! (hopefully to avoid unnecessary 412s).
//!
//! # Implementation
//!
//! The client side is implemented as reqwest middleware in [`client::CapClient`].
//! The server side is implemented as a plain struct that can be embedded in any server, in
//! `client::CapServer`.
//!
//! # TODO
//!
//! The eager `x-atuin-capabilities-available` path described above is not implemented yet: the
//! server only sends that header on a 412, so a stale client currently learns of a capability
//! change on its next rejected request rather than preemptively from an earlier response.

use parking_lot::RwLock;
use std::{any::Any, borrow::Borrow, cmp::Ordering, collections::BTreeSet, fmt};

pub mod http;

#[cfg(feature = "axum")]
pub mod axum;

mod all;
mod client;
mod middleware;
mod server;

pub use all::CapabilitiesCap;
pub use client::{CapClient, ServerSupportError};
pub use middleware::{CapMiddleware, CapMismatch, CapabilitiesExt};
pub use server::{CapServer, Negotiation};

/// A capability is always indexed by a String key.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, derive_more::AsRef)]
struct CapKey(String);

impl Borrow<str> for CapKey {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// A capability which two peers may negotiate.
///
/// The trait is dyn-compatible, so a heterogeneous set of capabilities can be stored and served as
/// `dyn Capability`. The type-level name lives in [`static_name`](Capability::static_name), gated on
/// `Self: Sized` so it stays off the vtable; [`name`](Capability::name) is its object-safe
/// counterpart, for when only a `dyn Capability` is in hand.
pub trait Capability: Any + Send + Sync {
    /// The name this capability is indexed by on the wire, eg `sh.atuin.server/records.batch`.
    fn static_name() -> &'static str
    where
        Self: Sized;

    /// The name of this capability. Implementers return [`Self::static_name`].
    fn name(&self) -> &'static str;

    /// Convert this capability's associated data into a JSON value.
    fn json(&self) -> Result<serde_json::Value, serde_json::Error>;
}

/// Recover the concrete type behind a stored capability.
///
/// `Capability: Any`, so a `&dyn Capability` upcasts to `&dyn Any`, which then downcasts to the
/// requested type -- yielding `None` if the stored capability is a different type.
fn downcast<C: Capability>(cap: &dyn Capability) -> Option<&C> {
    let cap: &dyn Any = cap;
    cap.downcast_ref::<C>()
}

/// A capability stored in a [`CapsBundle`], ordered and de-duplicated by its
/// [`name`](Capability::name).
///
/// `Box<dyn Capability>` is not `Ord`, so it cannot live in a `BTreeSet` directly. This newtype
/// compares purely by name -- the wire identifier, which is the set's real key -- and its
/// `Borrow<str>` lets the set be looked up by a bare name.
struct CapEntry(Box<dyn Capability>);

impl CapEntry {
    fn name(&self) -> &str {
        self.0.name()
    }
}

impl Borrow<str> for CapEntry {
    fn borrow(&self) -> &str {
        self.0.name()
    }
}

impl PartialEq for CapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.name() == other.name()
    }
}

impl Eq for CapEntry {}

impl PartialOrd for CapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name().cmp(other.name())
    }
}

/// Error from registering a capability whose name is already present in a [`CapsBundle`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a capability named {name:?} is already registered")]
pub struct DuplicateCapability {
    /// The name that was already registered.
    pub name: &'static str,
}

/// The capabilities a node advertises about itself.
#[derive(Default)]
pub struct CapsBundle {
    caps: RwLock<BTreeSet<CapEntry>>,
}

impl CapsBundle {
    /// Register a capability this node advertises.
    fn add<C: Capability>(&self, cap: C) -> Result<(), DuplicateCapability> {
        self.add_dyn(Box::new(cap))
    }

    /// Register an already type-erased capability this node advertises.
    ///
    /// Errors with [`DuplicateCapability`] if a capability with the same name is already present,
    /// leaving the existing one untouched.
    fn add_dyn(&self, cap: Box<dyn Capability>) -> Result<(), DuplicateCapability> {
        let name = cap.name();
        // `insert` reports whether the name was new; on a clash it keeps the existing entry.
        if self.caps.write().insert(CapEntry(cap)) {
            Ok(())
        } else {
            Err(DuplicateCapability { name })
        }
    }

    /// Check whether this node advertises the given capability.
    pub fn get<C: Capability + Clone>(&self) -> Option<C> {
        self.caps
            .read()
            .get(C::static_name())
            .and_then(|entry| downcast::<C>(entry.0.as_ref()))
            .cloned()
    }

    /// Serialize every advertised capability into a JSON object of name -> value.
    ///
    /// Keys are emitted in sorted order: the source is a `BTreeSet` ordered by name, so the object
    /// is byte-identical on every node running the same capability set -- which is what makes the
    /// server token stable.
    fn to_wire(&self) -> serde_json::Value {
        let object: serde_json::Map<String, serde_json::Value> = self
            .caps
            .read()
            .iter()
            .map(|entry| {
                let value = entry
                    .0
                    .json()
                    .expect("a capability value must be JSON-serializable");
                (entry.name().to_string(), value)
            })
            .collect();
        serde_json::Value::Object(object)
    }
}

impl fmt::Debug for CapsBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `dyn Any` is not `Debug`; show which capabilities are present, not their contents.
        f.debug_set()
            .entries(self.caps.read().iter().map(CapEntry::name))
            .finish()
    }
}
