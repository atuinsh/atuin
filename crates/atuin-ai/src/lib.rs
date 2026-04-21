// The FSM + driver architecture replaces the old dispatch/state system.
// Dead code from the old system (dispatch.rs, parts of state.rs, stream.rs,
// and tools/mod.rs) will be removed in a follow-up cleanup PR.
#![allow(dead_code)]

pub mod commands;
pub(crate) mod context;
pub(crate) mod context_window;
pub(crate) mod diff;
pub(crate) mod driver;
pub(crate) mod edit_permissions;
pub(crate) mod event_serde;
pub(crate) mod file_tracker;
pub(crate) mod fsm;
pub(crate) mod permissions;
pub(crate) mod session;
pub(crate) mod snapshots;
pub(crate) mod store;
pub(crate) mod stream;
pub(crate) mod tools;
pub(crate) mod tui;
