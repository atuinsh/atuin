//! Output stream filter for the PTY→stdout path.
//!
//! Processes the byte stream from the inner shell's PTY, forwarding normal
//! output to the real terminal while intercepting APC escape sequences in
//! our namespace (`atuin;`).
//!
//! # Protocol
//!
//! We use APC (Application Program Command) sequences, which ECMA-48 reserves
//! for application-private use. Most terminals silently ignore APC sequences
//! they don't recognise, so unhandled sequences are harmless.
//!
//! Wire format:
//!
//! ```text
//! ESC _ atuin; <verb> <space> <payload> ST
//! ```
//!
//! Where `ST` (String Terminator) is `ESC \` or `BEL` (0x07).
//!
//! Currently supported verbs:
//!
//! | Verb    | Direction     | Meaning                              |
//! |---------|---------------|--------------------------------------|
//! | `popup` | shell→proxy   | Run `<payload>` as a popup command   |
//! | `result`| proxy→shell   | Popup result fed back to the shell   |
//!
//! # Performance
//!
//! The common path (no ESC in a chunk) is a single `memchr` scan + `write_all`.
//! Byte-by-byte processing only activates when an ESC is encountered and stops
//! as soon as the sequence is resolved.

use std::io::Write;

use crate::osc133;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const ESC: u8 = 0x1B;
const BEL: u8 = 0x07;
const UNDERSCORE: u8 = b'_';
const BACKSLASH: u8 = b'\\';
const RIGHT_BRACKET: u8 = b']';

/// The prefix we expect after `ESC _` to claim an APC as ours.
const ATUIN_PREFIX: &[u8] = b"atuin;";

/// Maximum bytes we buffer while deciding if an escape sequence is ours.
/// `ESC _` (2) + `atuin;` (6) = 8 bytes before we can decide.
const HELD_CAP: usize = 16;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A request parsed from an intercepted APC sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Run `command` in a popup overlay.
    Popup { command: String },
}

// ---------------------------------------------------------------------------
// Filter state machine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Normal pass-through.
    Ground,
    /// Saw ESC — next byte decides what kind of sequence.
    Esc,
    /// Inside `ESC ]` — OSC sequence.  Forward to stdout AND osc133 parser.
    /// Tracking sub-state for the string terminator.
    OscBody,
    /// Inside OSC, saw ESC — might be `ESC \` (ST).
    OscEsc,
    /// Inside `ESC _` — APC sequence.  Accumulating prefix to decide if ours.
    ApcPrefix,
    /// Confirmed our APC — accumulating payload (not forwarded).
    ApcOurs,
    /// Inside our APC payload, saw ESC — might be ST.
    ApcOursEsc,
    /// APC that is NOT ours — forward bytes until ST.
    ApcOther,
    /// APC other, saw ESC — might be ST.
    ApcOtherEsc,
}

/// Streaming filter that sits between the inner PTY and real stdout.
///
/// Call [`Filter::push`] with each chunk read from the PTY.  Normal bytes are
/// written to the provided `Write` sink immediately.  Our APC sequences are
/// absorbed and returned as [`Request`] values.
pub struct Filter {
    state: State,
    /// OSC 133 zone tracker — fed transparently during OSC pass-through.
    pub osc133: osc133::Parser,
    /// Bytes held while we decide if an escape is ours.
    /// Only used in `Esc` and `ApcPrefix` states.
    held: [u8; HELD_CAP],
    held_len: usize,
    /// Payload accumulator for confirmed-ours APC sequences.
    /// Lazily allocated on first popup request.
    payload: Vec<u8>,
}

impl Filter {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            osc133: osc133::Parser::new(),
            held: [0u8; HELD_CAP],
            held_len: 0,
            payload: Vec::new(),
        }
    }

    /// Process a chunk of bytes from the inner PTY.
    ///
    /// Normal output is written to `out`.  If a complete APC popup request is
    /// found, it is returned.  At most one request is returned per call — if
    /// the chunk contains data after the APC, it is forwarded in the same call.
    pub fn push(&mut self, data: &[u8], out: &mut impl Write) -> Option<Request> {
        // Fast path: if we're in Ground state and there's no ESC in the
        // chunk, we can forward the entire thing in one syscall.
        if self.state == State::Ground {
            if let Some(esc_pos) = memchr(data, ESC) {
                // Forward everything before the ESC.
                if esc_pos > 0 {
                    self.osc133.push(&data[..esc_pos], |_| {});
                    let _ = out.write_all(&data[..esc_pos]);
                }
                // Process from the ESC onward byte-by-byte.
                return self.push_slow(&data[esc_pos..], out);
            } else {
                self.osc133.push(data, |_| {});
                let _ = out.write_all(data);
                return None;
            }
        }

        // We're mid-sequence from a previous chunk — go slow.
        self.push_slow(data, out)
    }

    /// Byte-by-byte processing.  Only called when an ESC has been encountered.
    fn push_slow(&mut self, data: &[u8], out: &mut impl Write) -> Option<Request> {
        let mut request: Option<Request> = None;

        // `clean_start` tracks the beginning of the next run of bytes that
        // should be forwarded to `out`.  We batch writes for efficiency.
        let mut clean_start: Option<usize> = None;

        for (i, &byte) in data.iter().enumerate() {
            match self.state {
                State::Ground => {
                    if byte == ESC {
                        // Flush any clean run before this ESC.
                        if let Some(start) = clean_start.take() {
                            let run = &data[start..i];
                            self.osc133.push(run, |_| {});
                            let _ = out.write_all(run);
                        }
                        self.held_len = 0;
                        self.hold(byte);
                        self.state = State::Esc;
                    } else if clean_start.is_none() {
                        clean_start = Some(i);
                    }
                }

                State::Esc => {
                    match byte {
                        UNDERSCORE => {
                            // ESC _ → APC.  Hold the byte and start prefix check.
                            self.hold(byte);
                            self.state = State::ApcPrefix;
                        }
                        RIGHT_BRACKET => {
                            // ESC ] → OSC.  Flush held ESC ] to output and parse.
                            self.hold(byte);
                            self.flush_held(out);
                            self.state = State::OscBody;
                        }
                        _ => {
                            // Some other escape (CSI, etc).  Flush held + this byte.
                            self.hold(byte);
                            self.flush_held(out);
                            self.state = State::Ground;
                            // This byte is part of the clean stream now, but
                            // we already flushed it via held.  Next clean byte
                            // starts fresh.
                        }
                    }
                }

                // --- OSC (pass-through + osc133 parsing) ---------------------

                State::OscBody => {
                    if clean_start.is_none() {
                        clean_start = Some(i);
                    }
                    if byte == BEL {
                        self.state = State::Ground;
                    } else if byte == ESC {
                        self.state = State::OscEsc;
                    }
                }

                State::OscEsc => {
                    if clean_start.is_none() {
                        clean_start = Some(i);
                    }
                    self.state = if byte == BACKSLASH {
                        State::Ground
                    } else if byte == ESC {
                        // Stay in OscEsc — this ESC might start ST.
                        State::OscEsc
                    } else {
                        State::OscBody
                    };
                }

                // --- APC prefix detection ------------------------------------

                State::ApcPrefix => {
                    if byte == BEL || (byte == BACKSLASH && self.held_ends_with_esc()) {
                        // APC terminated before we could decide — discard.
                        // (Extremely short APC, not ours.)
                        if byte == BACKSLASH {
                            // The ESC before this \ was held; don't flush it.
                            self.pop_held_esc();
                        }
                        self.flush_held(out);
                        self.state = State::Ground;
                    } else if byte == ESC {
                        // Could be start of ST (ESC \).  Hold it.
                        self.hold(byte);
                    } else {
                        self.hold(byte);
                        // Check if held bytes (after "ESC _") still match ATUIN_PREFIX.
                        let prefix_bytes = self.held_prefix_bytes();
                        let check_len = prefix_bytes.len();
                        if check_len <= ATUIN_PREFIX.len() {
                            if ATUIN_PREFIX[..check_len] == *prefix_bytes {
                                if check_len == ATUIN_PREFIX.len() {
                                    // Full prefix matched → it's ours!
                                    self.held_len = 0;
                                    self.payload.clear();
                                    self.state = State::ApcOurs;
                                }
                                // else: partial match, keep accumulating.
                            } else {
                                // Mismatch — not ours.  Flush held to stdout.
                                self.flush_held(out);
                                self.state = State::ApcOther;
                            }
                        } else {
                            // Exceeded prefix length without full match.
                            self.flush_held(out);
                            self.state = State::ApcOther;
                        }
                    }
                }

                // --- Our APC (absorb payload) --------------------------------

                State::ApcOurs => {
                    if byte == BEL {
                        request = self.dispatch_payload();
                        self.state = State::Ground;
                    } else if byte == ESC {
                        self.state = State::ApcOursEsc;
                    } else {
                        self.payload.push(byte);
                    }
                }

                State::ApcOursEsc => {
                    if byte == BACKSLASH {
                        request = self.dispatch_payload();
                        self.state = State::Ground;
                    } else {
                        // Not ST — ESC is part of payload.
                        self.payload.push(ESC);
                        if byte == ESC {
                            // Another ESC — stay in this state.
                        } else {
                            self.payload.push(byte);
                            self.state = State::ApcOurs;
                        }
                    }
                }

                // --- Other APC (forward transparently) -----------------------

                State::ApcOther => {
                    if clean_start.is_none() {
                        clean_start = Some(i);
                    }
                    if byte == BEL {
                        self.state = State::Ground;
                    } else if byte == ESC {
                        self.state = State::ApcOtherEsc;
                    }
                }

                State::ApcOtherEsc => {
                    if clean_start.is_none() {
                        clean_start = Some(i);
                    }
                    self.state = if byte == BACKSLASH {
                        State::Ground
                    } else if byte == ESC {
                        State::ApcOtherEsc
                    } else {
                        State::ApcOther
                    };
                }
            }
        }

        // Flush any remaining clean run.
        if let Some(start) = clean_start {
            let run = &data[start..];
            self.osc133.push(run, |_| {});
            let _ = out.write_all(run);
        }

        request
    }

    // -- helpers --

    #[inline]
    fn hold(&mut self, byte: u8) {
        if self.held_len < HELD_CAP {
            self.held[self.held_len] = byte;
            self.held_len += 1;
        }
    }

    /// Returns the held bytes *after* the `ESC _` header (i.e. the prefix
    /// candidate).
    #[inline]
    fn held_prefix_bytes(&self) -> &[u8] {
        // held[0] = ESC, held[1] = '_', prefix starts at [2]
        if self.held_len > 2 {
            &self.held[2..self.held_len]
        } else {
            &[]
        }
    }

    #[inline]
    fn held_ends_with_esc(&self) -> bool {
        self.held_len > 0 && self.held[self.held_len - 1] == ESC
    }

    /// Remove a trailing ESC from the held buffer (used when ESC turns out to
    /// be part of an ST rather than held data).
    #[inline]
    fn pop_held_esc(&mut self) {
        if self.held_ends_with_esc() {
            self.held_len -= 1;
        }
    }

    /// Write all held bytes to `out` and feed them to the osc133 parser.
    fn flush_held(&mut self, out: &mut impl Write) {
        if self.held_len > 0 {
            let bytes = &self.held[..self.held_len];
            self.osc133.push(bytes, |_| {});
            let _ = out.write_all(bytes);
            self.held_len = 0;
        }
    }

    /// Parse the accumulated payload into a [`Request`].
    fn dispatch_payload(&mut self) -> Option<Request> {
        let payload = &self.payload;

        // Expected format: "popup <command>" (verb, space, rest)
        if let Some(rest) = payload.strip_prefix(b"popup ") {
            let command = String::from_utf8_lossy(rest).into_owned();
            if !command.is_empty() {
                return Some(Request::Popup { command });
            }
        }

        None
    }
}

/// Find the first occurrence of `needle` in `haystack`.
#[inline]
fn memchr(haystack: &[u8], needle: u8) -> Option<usize> {
    haystack.iter().position(|&b| b == needle)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: push data through a filter and return (written output, request).
    fn filter(data: &[u8]) -> (Vec<u8>, Option<Request>) {
        let mut f = Filter::new();
        let mut out = Vec::new();
        let req = f.push(data, &mut out);
        (out, req)
    }

    /// Helper: push data in single-byte increments (worst case).
    fn filter_bytewise(data: &[u8]) -> (Vec<u8>, Option<Request>) {
        let mut f = Filter::new();
        let mut out = Vec::new();
        let mut req = None;
        for &b in data {
            if let Some(r) = f.push(&[b], &mut out) {
                req = Some(r);
            }
        }
        (out, req)
    }

    // -- plain text passes through -------------------------------------------

    #[test]
    fn plain_text() {
        let (out, req) = filter(b"hello world\r\n");
        assert_eq!(out, b"hello world\r\n");
        assert!(req.is_none());
    }

    // -- normal escape sequences pass through --------------------------------

    #[test]
    fn csi_passes_through() {
        let data = b"\x1b[32mgreen\x1b[0m";
        let (out, req) = filter(data);
        assert_eq!(out, data.as_slice());
        assert!(req.is_none());
    }

    #[test]
    fn osc_title_passes_through() {
        let data = b"\x1b]0;my title\x07rest";
        let (out, req) = filter(data);
        assert_eq!(out, data.as_slice());
        assert!(req.is_none());
    }

    #[test]
    fn osc_133_passes_through() {
        let data = b"prompt\x1b]133;A\x07$ ";
        let (out, req) = filter(data);
        assert_eq!(out, data.as_slice());
        assert!(req.is_none());
    }

    // -- our APC is intercepted ----------------------------------------------

    #[test]
    fn popup_apc_intercepted_bel() {
        let data = b"before\x1b_atuin;popup atuin search -i\x07after";
        let (out, req) = filter(data);
        assert_eq!(out, b"beforeafter");
        assert_eq!(
            req,
            Some(Request::Popup {
                command: "atuin search -i".into()
            })
        );
    }

    #[test]
    fn popup_apc_intercepted_st() {
        let data = b"\x1b_atuin;popup atuin search -i\x1b\\trailing";
        let (out, req) = filter(data);
        assert_eq!(out, b"trailing");
        assert_eq!(
            req,
            Some(Request::Popup {
                command: "atuin search -i".into()
            })
        );
    }

    #[test]
    fn popup_apc_bytewise() {
        let data = b"\x1b_atuin;popup cmd\x07";
        let (out, req) = filter_bytewise(data);
        assert_eq!(out, b"");
        assert_eq!(
            req,
            Some(Request::Popup {
                command: "cmd".into()
            })
        );
    }

    // -- non-atuin APC passes through ----------------------------------------

    #[test]
    fn foreign_apc_passes_through() {
        let data = b"\x1b_other;stuff\x07rest";
        let (out, req) = filter(data);
        assert_eq!(out, data.as_slice());
        assert!(req.is_none());
    }

    // -- split across chunks -------------------------------------------------

    #[test]
    fn popup_split_at_prefix() {
        let mut f = Filter::new();
        let mut out = Vec::new();

        assert!(f.push(b"\x1b_atu", &mut out).is_none());
        assert_eq!(out, b""); // held, not flushed yet

        let req = f.push(b"in;popup cmd\x07done", &mut out);
        assert_eq!(out, b"done");
        assert_eq!(
            req,
            Some(Request::Popup {
                command: "cmd".into()
            })
        );
    }

    #[test]
    fn popup_split_at_esc() {
        let mut f = Filter::new();
        let mut out = Vec::new();

        assert!(f.push(b"hello\x1b", &mut out).is_none());
        assert_eq!(out, b"hello");

        let req = f.push(b"_atuin;popup test\x07", &mut out);
        assert_eq!(
            req,
            Some(Request::Popup {
                command: "test".into()
            })
        );
    }

    // -- empty / edge cases --------------------------------------------------

    #[test]
    fn empty_input() {
        let (out, req) = filter(b"");
        assert!(out.is_empty());
        assert!(req.is_none());
    }

    #[test]
    fn empty_popup_command_ignored() {
        let data = b"\x1b_atuin;popup \x07";
        let (out, req) = filter(data);
        assert_eq!(out, b"");
        // Empty command after "popup " is ignored.
        assert!(req.is_none());
    }

    #[test]
    fn unknown_atuin_verb_ignored() {
        let data = b"\x1b_atuin;unknown stuff\x07";
        let (out, req) = filter(data);
        assert_eq!(out, b"");
        assert!(req.is_none());
    }

    // -- OSC 133 still tracked -----------------------------------------------

    #[test]
    fn osc133_zone_tracked_through_filter() {
        let mut f = Filter::new();
        let mut out = Vec::new();
        f.push(b"\x1b]133;A\x07", &mut out);
        assert_eq!(f.osc133.zone(), osc133::Zone::Prompt);
        f.push(b"\x1b]133;B\x07", &mut out);
        assert_eq!(f.osc133.zone(), osc133::Zone::Input);
    }

    // -- mixed sequences in one chunk ----------------------------------------

    #[test]
    fn mixed_osc_and_apc() {
        let data = b"\x1b]133;A\x07prompt\x1b_atuin;popup cmd\x07\x1b]133;B\x07input";
        let mut f = Filter::new();
        let mut out = Vec::new();
        let req = f.push(data, &mut out);

        assert_eq!(out, b"\x1b]133;A\x07prompt\x1b]133;B\x07input");
        assert_eq!(
            req,
            Some(Request::Popup {
                command: "cmd".into()
            })
        );
        assert_eq!(f.osc133.zone(), osc133::Zone::Input);
    }
}
