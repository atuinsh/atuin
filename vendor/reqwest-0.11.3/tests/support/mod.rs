pub mod server;

// TODO: remove once done converting to new support server?
#[allow(unused)]
pub static DEFAULT_USER_AGENT: &'static str =
    concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
