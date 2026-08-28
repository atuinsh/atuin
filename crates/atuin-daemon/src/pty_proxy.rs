//! Operations for interacting with the atuin-pty-proxy.

use atuin_pty_proxy::IpcClient as PtyClient;
use dashmap::DashMap;

/// Unique identifier to an active `pty-proxy`.
pub struct PtyProxyId;

/// A pool of connections to **different** pty-proxies.
#[derive(Debug)]
pub struct PtyProxyPool {
    connections: DashMap<PtyProxyId, PtyClient>,
}

impl PtyProxyPool {
    /// Get or create a connection to a pty-proxy, given the identifier of the pty-proxy.
    pub fn get_or_create(&self) -> &PtyClient {}
}
