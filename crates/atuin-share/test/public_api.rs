//! Integration tests for `atuin-share`'s public API.
//!
//! Per project convention, tests live in `test/`, never in `src/`. Because a
//! Cargo test target is a separate crate, only the crate's public surface is
//! reachable here — `run_share`, `ShareOptions`, `Size`. The internals
//! (transport, session, compositor, keyframes, backpressure, …) are private and
//! are exercised only through this public surface.

use atuin_share::{ShareOptions, Size};

#[test]
fn size_is_copy_and_eq() {
    let a = Size { cols: 80, rows: 24 };
    let b = a;
    assert_eq!(a, b);
}

#[test]
fn share_options_round_trip() {
    let o = ShareOptions {
        write: true,
        hub_url: "wss://h".into(),
        api_token: "tok".into(),
    };
    assert!(o.write);
    assert_eq!(o.hub_url, "wss://h");
    assert_eq!(o.api_token, "tok");
}

/// `run_share` is unix-only (it needs a PTY) and refuses to run without an
/// interactive terminal. A test binary's stdin/stdout are not a tty, so this
/// exercises exactly that guard.
#[cfg(unix)]
#[test]
fn refuses_when_not_a_tty() {
    let err = atuin_share::run_share(ShareOptions {
        write: false,
        hub_url: "wss://x".into(),
        api_token: "t".into(),
    })
    .unwrap_err();
    assert!(format!("{err:#}").contains("terminal"));
}
