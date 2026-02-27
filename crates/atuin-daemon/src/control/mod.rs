//! Control module for external event injection.
//!
//! This module provides the gRPC service that allows external processes
//! (like CLI commands) to inject events into the daemon's event bus.

mod service;

// Include the generated proto code
tonic::include_proto!("control");

// Re-export the service
pub use service::ControlService;
