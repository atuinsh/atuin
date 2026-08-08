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

/// The client history "packfile" feature, expressed as a capability.
///
/// A server that can store and serve packfile bundles (`/api/v0/bundles`) advertises this, so a
/// client only publishes/consumes packfiles a server has confirmed it supports; otherwise the
/// client degrades to loose-record history sync (lossless -- the packer never deletes history
/// rows).
///
/// Home note: this cap deliberately lives here in atuin-domain, next to [`CapabilitiesCap`], rather
/// than with its feature in atuin-client/src/packfile -- overriding the module-level ideal that
/// "feature-specific ones live with their features". The reason is ownership of the wire contract:
/// atuin-server advertises this cap and cannot depend on atuin-client in production (atuin-client is
/// only a dev-dependency there), so the shared vocabulary crate is the only honest home for a type
/// both sides must agree on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackfileCap {
    /// The version of the packfile protocol the server implements.
    pub version: u32,
}

impl Capability for PackfileCap {
    fn static_name() -> &'static str {
        "sh.atuin.server/records.bundle"
    }

    fn name(&self) -> &'static str {
        Self::static_name()
    }

    fn json(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::to_value(self)
    }
}

#[cfg(test)]
mod tests {
    use super::PackfileCap;
    use crate::caps::Capability;

    #[test]
    fn packfile_cap_wire_identity() {
        // The CRI is the wire contract between a server that advertises the cap and a client that
        // reads it -- pin it so a rename can never silently disable the gate.
        assert_eq!(PackfileCap::static_name(), "sh.atuin.server/records.bundle");
        // The value round-trips to the same `{ "version": u32 }` shape as CapabilitiesCap.
        assert_eq!(
            PackfileCap { version: 1 }.json().unwrap(),
            serde_json::json!({ "version": 1 })
        );
    }
}
