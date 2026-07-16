// TODO(v2 port): remove in the deletion-audit slice. The FSM's effect
// layer (stream bridge, tool execution, permissions, usage) is dormant
// until `AiApp::update` reconnects it slice by slice; this silences the
// transitional dead-code warnings without touching those files.
#![allow(dead_code)]

pub mod commands;
pub(crate) mod context;
pub(crate) mod context_window;
pub(crate) mod diff;
pub(crate) mod edit_permissions;
pub(crate) mod event_serde;
pub(crate) mod file_tracker;
pub(crate) mod fsm;
pub(crate) mod history_format;
pub mod mcp;
pub(crate) mod models;
pub(crate) mod permissions;
pub(crate) mod session;
pub(crate) mod skills;
pub(crate) mod snapshots;
pub(crate) mod store;
pub(crate) mod stream;
pub(crate) mod tools;
pub(crate) mod tui;
pub(crate) mod usage;
pub(crate) mod user_context;
