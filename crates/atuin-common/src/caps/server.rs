use std::sync::Arc;

use super::{Capability, OwnCaps};

/// Server-side capability set: advertises its own capabilities and nothing more.
///
/// Cloning is cheap.
#[derive(Debug, Clone, Default)]
pub struct CapServer {
    own: Arc<OwnCaps>,
}

impl CapServer {
    /// Register a capability this server advertises.
    pub fn can<C: Capability>(&self, cap: C) {
        self.own.can(cap);
    }

    /// Check whether this server advertises the given capability.
    pub fn support<C: Capability + Clone>(&self) -> Option<C> {
        self.own.support()
    }
}
