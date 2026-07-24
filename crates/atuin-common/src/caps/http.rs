//! HTTP header names for the capability-negotiation protocol.
//!
//! Shared so the client middleware and the (future) server side agree on one spelling.

/// Request header: the token the client currently knows, echoed back to the server verbatim.
pub const KNOWN_HEADER: &str = "x-atuin-capabilities-known";

/// Response header: the token the server actually has.
pub const AVAILABLE_HEADER: &str = "x-atuin-capabilities-available";
