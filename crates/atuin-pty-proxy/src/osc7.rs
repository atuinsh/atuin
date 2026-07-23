//! Streaming parser for OSC 7 (current working directory) escape sequences.
//!
//! OSC 7 is the de-facto standard for shells to inform the terminal of their
//! current working directory:
//!
//! ```text
//! ESC ] 7 ; file://<hostname>/<path> ST
//! ```
//!
//! ST is either BEL (0x07) or ESC \ (0x1B 0x5C).  The path is percent-encoded
//! per RFC 3986 (e.g. spaces as %20).
//!
//! atuin pty-proxy parses this from the inner shell's output and updates its
//! own cwd to match, so that terminals/multiplexers (tmux, Ghostty, Kitty,
//! etc.) that read pane cwd via process introspection see the inner shell's
//! directory rather than the proxy's startup directory.
//!
//! # Design goals
//!
//! Mirrors `osc133.rs`: streaming, transparent (caller still forwards bytes).
//! The parameter buffer is pre-sized to its hard cap so steady-state parsing
//! does not allocate.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode};
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

/// RFC 3986 path encode set: percent-encode everything that is not in the
/// `unreserved` set (`A-Za-z0-9-._~`) or the path separator `/`.
pub const PATH_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/');

/// Event emitted when an OSC 7 sequence is fully parsed.  The host portion
/// of the URI is intentionally discarded — every consumer in atuin pty-proxy
/// only cares about the cwd path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CwdEvent {
    pub path: PathBuf,
}

const ESC: u8 = 0x1B;
const BEL: u8 = 0x07;
const BACKSLASH: u8 = b'\\';
const RIGHT_BRACKET: u8 = b']';

/// Cap on the buffered OSC parameter payload.  Generous enough for any
/// realistic file path including percent-encoding overhead.
const PARAM_BUF_CAP: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    OscParam,
    OscEsc,
}

/// Streaming parser.  Feed arbitrary byte slices via [`Parser::push`]; the
/// parser invokes `on_event` for every fully-formed OSC 7 sequence.
pub struct Parser {
    state: State,
    param_buf: Vec<u8>,
    overflowed: bool,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            param_buf: Vec::with_capacity(PARAM_BUF_CAP),
            overflowed: false,
        }
    }

    pub fn push(&mut self, data: &[u8], mut on_event: impl FnMut(CwdEvent)) {
        for &byte in data {
            match self.state {
                State::Ground => {
                    if byte == ESC {
                        self.state = State::Esc;
                    }
                }
                State::Esc => {
                    if byte == RIGHT_BRACKET {
                        self.state = State::OscParam;
                        self.param_buf.clear();
                        self.overflowed = false;
                    } else {
                        self.state = State::Ground;
                    }
                }
                State::OscParam => {
                    if byte == BEL {
                        self.dispatch(&mut on_event);
                        self.state = State::Ground;
                    } else if byte == ESC {
                        self.state = State::OscEsc;
                    } else if self.param_buf.len() < PARAM_BUF_CAP {
                        self.param_buf.push(byte);
                    } else {
                        self.overflowed = true;
                    }
                }
                State::OscEsc => {
                    if byte == BACKSLASH {
                        self.dispatch(&mut on_event);
                    }
                    self.state = State::Ground;
                }
            }
        }
    }

    fn dispatch(&mut self, on_event: &mut impl FnMut(CwdEvent)) {
        let overflowed = std::mem::replace(&mut self.overflowed, false);
        if overflowed {
            return;
        }
        let params = &self.param_buf[..];
        // Must start with "7;"
        if params.len() < 2 || params[0] != b'7' || params[1] != b';' {
            return;
        }
        if let Some(event) = parse_file_uri(&params[2..]) {
            on_event(event);
        }
    }
}

/// Parse a `file://[host]/<path>` URI into a decoded filesystem path.  The
/// host portion is parsed and discarded.  Returns None on malformed input
/// or on a non-absolute path (OSC 7 paths are absolute by definition).
fn parse_file_uri(uri: &[u8]) -> Option<CwdEvent> {
    let s = std::str::from_utf8(uri).ok()?;
    let s = s.strip_prefix("file://")?;
    // Skip past the host (everything up to the first '/').  An empty host
    // (`file:///path`) is valid; the path begins at that first '/'.
    let path = &s[s.find('/')?..];
    let bytes = percent_decode(path.as_bytes()).collect::<Vec<u8>>();
    let path = PathBuf::from(OsString::from_vec(bytes));
    // OSC 7 paths are absolute by definition, and a wire payload with `..`
    // components is either malformed or an attempt by attacker-controlled
    // pty output (e.g. `cat /random/file`) to redirect the daemon's cwd
    // somewhere unexpected.  Reject both.
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return None;
    }
    Some(CwdEvent { path })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_events(data: &[u8]) -> Vec<CwdEvent> {
        let mut parser = Parser::new();
        let mut events = Vec::new();
        parser.push(data, |e| events.push(e));
        events
    }

    #[test]
    fn simple_path_bel() {
        let events = parse_events(b"\x1b]7;file://host/tmp\x07");
        assert_eq!(
            events,
            vec![CwdEvent {
                path: PathBuf::from("/tmp"),
            }]
        );
    }

    #[test]
    fn simple_path_st() {
        let events = parse_events(b"\x1b]7;file://host/tmp\x1b\\");
        assert_eq!(
            events,
            vec![CwdEvent {
                path: PathBuf::from("/tmp"),
            }]
        );
    }

    #[test]
    fn empty_hostname() {
        let events = parse_events(b"\x1b]7;file:///home/x\x07");
        assert_eq!(
            events,
            vec![CwdEvent {
                path: PathBuf::from("/home/x"),
            }]
        );
    }

    #[test]
    fn percent_decode_spaces() {
        let events = parse_events(b"\x1b]7;file:///home/with%20space\x07");
        assert_eq!(
            events,
            vec![CwdEvent {
                path: PathBuf::from("/home/with space"),
            }]
        );
    }

    #[test]
    fn percent_decode_non_ascii() {
        // /тест URL-encoded as UTF-8
        let events = parse_events(b"\x1b]7;file:///%D1%82%D0%B5%D1%81%D1%82\x07");
        assert_eq!(
            events,
            vec![CwdEvent {
                path: PathBuf::from("/тест"),
            }]
        );
    }

    #[test]
    fn split_across_chunks() {
        let mut parser = Parser::new();
        let mut events = Vec::new();
        parser.push(b"\x1b]7;file:", |e| events.push(e));
        parser.push(b"//host/tmp", |e| events.push(e));
        parser.push(b"\x07", |e| events.push(e));
        assert_eq!(
            events,
            vec![CwdEvent {
                path: PathBuf::from("/tmp"),
            }]
        );
    }

    #[test]
    fn ignores_other_osc_numbers() {
        let events = parse_events(b"\x1b]133;A\x07");
        assert!(events.is_empty());
    }

    #[test]
    fn ignores_malformed_uri() {
        let events = parse_events(b"\x1b]7;not-a-file-uri\x07");
        assert!(events.is_empty());
    }

    #[test]
    fn over_long_payload_is_dropped() {
        let mut data = b"\x1b]7;file:///".to_vec();
        data.extend(std::iter::repeat(b'a').take(PARAM_BUF_CAP * 2));
        data.extend_from_slice(b"\x07");
        let events = parse_events(&data);
        assert!(events.is_empty(), "over-long OSC 7 should be dropped");
    }

    #[test]
    fn rejects_relative_path() {
        // Wire payload with no leading slash after host: malformed.
        let events = parse_events(b"\x1b]7;file://hostrelative\x07");
        assert!(events.is_empty());
    }

    #[test]
    fn rejects_parent_dir_traversal() {
        // Defends against `cat /attacker/file` redirecting the daemon's cwd.
        let events = parse_events(b"\x1b]7;file:///a/../../etc\x07");
        assert!(events.is_empty());
    }
}
