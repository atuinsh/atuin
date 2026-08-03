use serde::{Deserialize, Serialize};

use super::Capability;

/// The capability-negotiation protocol itself, expressed as a capability.
///
/// A server that speaks capabilities advertises this, so a client can observe -- from the
/// capability set alone -- that the protocol is supported, and at which version. It is
/// deliberately self-referential: receiving any capability document already implies the server
/// understands capabilities. Naming that fact gives the negotiation machinery a concrete
/// capability to carry today, while the richer feature-specific ones live with their features.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitiesCap {
    /// The version of the capability-negotiation protocol the server implements.
    pub version: u32,
}

impl Capability for CapabilitiesCap {
    fn static_name() -> &'static str {
        "sh.atuin.server/capabilities"
    }

    fn name(&self) -> &'static str {
        Self::static_name()
    }

    fn json(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::to_value(self)
    }
}
