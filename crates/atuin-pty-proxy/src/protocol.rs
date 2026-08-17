//! Subscriber protocol for the pty-proxy Unix socket.
//!
//! The proxy serves two kinds of client on the same socket:
//!
//! * **Legacy one-shot** (the search popup): the client connects, writes
//!   nothing, and reads a single [`Snapshot`] blob until EOF. This path
//!   predates this module and its bytes are frozen.
//! * **Framed v2** (`atuin lab share --active` and future subscribers): the
//!   client leads with the 4-byte magic [`MAGIC`], then both directions speak
//!   length-prefixed frames.
//!
//! The server tells them apart with a greeting sniff — see
//! [`classify_greeting`]. The magic carries the protocol major version
//! (`APX1`); [`PROTOCOL_VERSION`] in the Hello frame carries the minor.
//!
//! # Framing
//!
//! Every frame, in both directions, is:
//!
//! ```text
//! [type: u8][len: u32 BE][payload: len bytes]
//! ```
//!
//! `len` is capped at [`MAX_FRAME_LEN`]; an oversize frame is a protocol
//! error and the peer closes the connection.
//!
//! Client -> server:
//!
//! * `Subscribe` (0x01): `[flags: u8][token_len: u16 BE][token bytes]` —
//!   flags bit 0 requests input access. Exactly one Subscribe per
//!   connection, and it must be the first frame.
//! * `Input` (0x02): raw bytes for the PTY. Only honoured when the Hello
//!   granted input; otherwise the server closes the connection.
//!
//! Server -> client:
//!
//! * `Hello` (0x81): `[version: u8][input_granted: u8]`.
//! * `Keyframe` (0x82): a [`Snapshot`] blob — the same encoding the legacy
//!   one-shot path serves.
//! * `Output` (0x83): raw PTY output bytes.
//! * `Resize` (0x84): `[rows: u16 BE][cols: u16 BE]`.
//! * `End` (0x85): empty; best-effort notice before close. Socket EOF is
//!   the authoritative end-of-session signal.
//!
//! Order: Subscribe -> Hello -> Keyframe -> interleaved Output/Resize in the
//! order the proxy's parser thread observed them. A server receiving an
//! unknown client frame closes the connection; a client receiving an unknown
//! server frame should skip it (forward compatibility).
//!
//! # Trust boundary
//!
//! Same-uid is the trust boundary. A process running as the same user can
//! read the token file next to the socket, and could equally ptrace the
//! shell or write to the user's tty directly. The token, the 0700 socket
//! directory, and the server-side peer-uid check defend against *other*
//! users, against the socket path leaking into logs or environment dumps,
//! and against accidental cross-user access — not against malware already
//! running as the user.

use std::io::{self, Read, Write as _};

/// Greeting bytes a framed-v2 client writes immediately after connecting.
/// The trailing digit is the protocol major version.
pub const MAGIC: [u8; 4] = *b"APX1";

/// Protocol minor version, carried in the Hello frame.
pub const PROTOCOL_VERSION: u8 = 1;

/// Maximum frame payload length. Oversize frames are a protocol error.
pub const MAX_FRAME_LEN: usize = 1024 * 1024;

/// Client -> server: subscribe to the session feed. First frame, exactly once.
pub const FRAME_SUBSCRIBE: u8 = 0x01;
/// Client -> server: raw bytes for the PTY (requires granted input).
pub const FRAME_INPUT: u8 = 0x02;
/// Server -> client: protocol version and whether input was granted.
pub const FRAME_HELLO: u8 = 0x81;
/// Server -> client: a [`Snapshot`] blob of the current screen.
pub const FRAME_KEYFRAME: u8 = 0x82;
/// Server -> client: raw PTY output bytes.
pub const FRAME_OUTPUT: u8 = 0x83;
/// Server -> client: the terminal was resized.
pub const FRAME_RESIZE: u8 = 0x84;
/// Server -> client: best-effort end-of-session notice before close.
pub const FRAME_END: u8 = 0x85;

/// Subscribe flags, bit 0: the client wants to send [`FRAME_INPUT`] frames.
pub const SUBSCRIBE_FLAG_WANT_INPUT: u8 = 0x01;

/// Which protocol a freshly accepted connection speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Greeting {
    /// No greeting (timeout/EOF) or wrong bytes: serve the legacy one-shot
    /// snapshot, byte-identical to the pre-v2 server.
    Legacy,
    /// The client sent [`MAGIC`]: speak framed v2.
    V2,
}

/// Classify the greeting bytes read during the post-accept sniff.
///
/// `first` is whatever the sniff managed to read before its timeout — fewer
/// than four bytes (the legacy popup writes nothing at all) or a mismatch
/// both mean legacy.
#[must_use]
pub fn classify_greeting(first: &[u8]) -> Greeting {
    if first == MAGIC {
        Greeting::V2
    } else {
        Greeting::Legacy
    }
}

/// Encode a frame header + payload.
///
/// # Panics
///
/// Panics if `payload` exceeds [`MAX_FRAME_LEN`]. Callers own the payload
/// sizes (PTY read chunks and screen snapshots) and must cap them first.
#[must_use]
pub fn encode_frame(frame_type: u8, payload: &[u8]) -> Vec<u8> {
    assert!(
        payload.len() <= MAX_FRAME_LEN,
        "frame payload exceeds MAX_FRAME_LEN"
    );
    let len = u32::try_from(payload.len()).expect("payload length fits in u32");
    let mut buf = Vec::with_capacity(5 + payload.len());
    buf.push(frame_type);
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(payload);
    buf
}

/// Read one frame. Returns `Ok(None)` on a clean EOF at a frame boundary.
///
/// Unknown frame types are returned as-is: the transport layer does not
/// decide policy (the server closes on unknown client frames; clients skip
/// unknown server frames for forward compatibility).
///
/// # Errors
///
/// Fails on EOF mid-frame, on a length above [`MAX_FRAME_LEN`], or on any
/// underlying read error.
pub fn read_frame(reader: &mut impl Read) -> io::Result<Option<(u8, Vec<u8>)>> {
    let mut header = [0u8; 5];
    if !read_exact_or_eof(reader, &mut header)? {
        return Ok(None);
    }
    let frame_type = header[0];
    let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if len > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame exceeds maximum length",
        ));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    Ok(Some((frame_type, payload)))
}

/// Fill `buf` completely. Returns `Ok(false)` on EOF before the first byte,
/// `Ok(true)` when full; EOF partway through is an [`io::ErrorKind::UnexpectedEof`].
fn read_exact_or_eof(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => {
                if filled == 0 {
                    return Ok(false);
                }
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "eof in the middle of a frame",
                ));
            }
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

/// Build a complete Subscribe frame.
///
/// # Panics
///
/// Panics if `token` is longer than `u16::MAX` bytes; real tokens are 64
/// ASCII characters.
#[must_use]
pub fn subscribe_frame(want_input: bool, token: &[u8]) -> Vec<u8> {
    let token_len = u16::try_from(token.len()).expect("token length fits in u16");
    let mut payload = Vec::with_capacity(3 + token.len());
    payload.push(if want_input {
        SUBSCRIBE_FLAG_WANT_INPUT
    } else {
        0
    });
    payload.extend_from_slice(&token_len.to_be_bytes());
    payload.extend_from_slice(token);
    encode_frame(FRAME_SUBSCRIBE, &payload)
}

/// Decode a Subscribe payload into `(want_input, token)`.
#[must_use]
pub fn decode_subscribe(payload: &[u8]) -> Option<(bool, &[u8])> {
    if payload.len() < 3 {
        return None;
    }
    let want_input = payload[0] & SUBSCRIBE_FLAG_WANT_INPUT != 0;
    let token_len = u16::from_be_bytes([payload[1], payload[2]]) as usize;
    if payload.len() != 3 + token_len {
        return None;
    }
    Some((want_input, &payload[3..]))
}

/// Build a complete Hello frame.
#[must_use]
pub fn hello_frame(input_granted: bool) -> Vec<u8> {
    encode_frame(FRAME_HELLO, &[PROTOCOL_VERSION, u8::from(input_granted)])
}

/// Decode a Hello payload into `(version, input_granted)`. Trailing bytes
/// are tolerated so future minors can extend the frame.
#[must_use]
pub fn decode_hello(payload: &[u8]) -> Option<(u8, bool)> {
    if payload.len() < 2 {
        return None;
    }
    Some((payload[0], payload[1] != 0))
}

/// Build a complete Input frame.
///
/// # Panics
///
/// Panics if `data` exceeds [`MAX_FRAME_LEN`].
#[must_use]
pub fn input_frame(data: &[u8]) -> Vec<u8> {
    encode_frame(FRAME_INPUT, data)
}

/// Build a complete Resize frame.
#[must_use]
pub fn resize_frame(rows: u16, cols: u16) -> Vec<u8> {
    let mut payload = [0u8; 4];
    payload[..2].copy_from_slice(&rows.to_be_bytes());
    payload[2..].copy_from_slice(&cols.to_be_bytes());
    encode_frame(FRAME_RESIZE, &payload)
}

/// Decode a Resize payload into `(rows, cols)`.
#[must_use]
pub fn decode_resize(payload: &[u8]) -> Option<(u16, u16)> {
    if payload.len() != 4 {
        return None;
    }
    Some((
        u16::from_be_bytes([payload[0], payload[1]]),
        u16::from_be_bytes([payload[2], payload[3]]),
    ))
}

/// Build a complete End frame (empty payload).
#[must_use]
pub fn end_frame() -> Vec<u8> {
    encode_frame(FRAME_END, &[])
}

/// A decoded screen snapshot.
///
/// Wire encoding (identical to the legacy one-shot reply and the Keyframe
/// payload — the search popup's hand-rolled decoder depends on these bytes):
///
/// ```text
/// [rows: u16 BE][cols: u16 BE][cursor_row: u16 BE][cursor_col: u16 BE]
/// [row_0_len: u32 BE][row_0_bytes...]
/// [row_1_len: u32 BE][row_1_bytes...]
/// ...
/// ```
///
/// Each row's bytes come from vt100's `rows_formatted(0, cols)` and contain
/// pre-built ANSI escape sequences: a client can write them straight to a
/// terminal without its own vt100 parser. `decode` does not require
/// `rows_data.len() == rows`; consumers iterate `rows_data`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Screen height in rows.
    pub rows: u16,
    /// Screen width in columns.
    pub cols: u16,
    /// Cursor row, 0-based.
    pub cursor_row: u16,
    /// Cursor column, 0-based.
    pub cursor_col: u16,
    /// Pre-formatted ANSI bytes for each screen row.
    pub rows_data: Vec<Vec<u8>>,
}

impl Snapshot {
    /// Capture a snapshot of a vt100 screen.
    #[must_use]
    pub fn from_screen(screen: &vt100::Screen) -> Self {
        let (rows, cols) = screen.size();
        let (cursor_row, cursor_col) = screen.cursor_position();
        let rows_data = screen.rows_formatted(0, cols).collect();
        Self {
            rows,
            cols,
            cursor_row,
            cursor_col,
            rows_data,
        }
    }

    /// Encode to the wire format above.
    ///
    /// # Panics
    ///
    /// Panics if a single row's formatted bytes exceed `u32::MAX` — not
    /// reachable for any real terminal.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::with_capacity(256 + (self.rows as usize * self.cols as usize));
        buf.extend_from_slice(&self.rows.to_be_bytes());
        buf.extend_from_slice(&self.cols.to_be_bytes());
        buf.extend_from_slice(&self.cursor_row.to_be_bytes());
        buf.extend_from_slice(&self.cursor_col.to_be_bytes());

        for row_bytes in &self.rows_data {
            let len = u32::try_from(row_bytes.len()).expect("row length fits in u32");
            buf.extend_from_slice(&len.to_be_bytes());
            buf.extend_from_slice(row_bytes);
        }

        buf
    }

    /// Decode from the wire format. Returns `None` on truncated or
    /// malformed input.
    #[must_use]
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < 8 {
            return None;
        }
        let rows = u16::from_be_bytes([data[0], data[1]]);
        let cols = u16::from_be_bytes([data[2], data[3]]);
        let cursor_row = u16::from_be_bytes([data[4], data[5]]);
        let cursor_col = u16::from_be_bytes([data[6], data[7]]);

        let mut rows_data = Vec::with_capacity(rows as usize);
        let mut offset = 8usize;
        while offset < data.len() {
            let len_bytes = data.get(offset..offset + 4)?;
            let row_len = u32::from_be_bytes(len_bytes.try_into().ok()?) as usize;
            offset += 4;
            let row = data.get(offset..offset + row_len)?;
            rows_data.push(row.to_vec());
            offset += row_len;
        }

        Some(Self {
            rows,
            cols,
            cursor_row,
            cursor_col,
            rows_data,
        })
    }
}

/// Render a snapshot as a full-screen repaint: clear screen + home, each row
/// at its absolute position, an SGR reset, then a CUP restoring the cursor.
///
/// The result is suitable to feed into any ANSI terminal (or terminal model)
/// as "the screen now looks exactly like this".
#[must_use]
pub fn snapshot_to_ansi(snapshot: &Snapshot) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"\x1b[H\x1b[2J");
    for (i, row) in snapshot.rows_data.iter().enumerate() {
        // Rows are positioned absolutely; CUP is 1-based.
        let _ = write!(out, "\x1b[{};1H", i + 1);
        out.extend_from_slice(row);
    }
    out.extend_from_slice(b"\x1b[0m");
    let _ = write!(
        out,
        "\x1b[{};{}H",
        u32::from(snapshot.cursor_row) + 1,
        u32::from(snapshot.cursor_col) + 1
    );
    out
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn frame_roundtrip() {
        let frame = encode_frame(FRAME_OUTPUT, b"hello");
        let mut cursor = Cursor::new(frame);
        let (frame_type, payload) = read_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(frame_type, FRAME_OUTPUT);
        assert_eq!(payload, b"hello");
        assert!(read_frame(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn empty_payload_roundtrip() {
        let mut cursor = Cursor::new(end_frame());
        let (frame_type, payload) = read_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(frame_type, FRAME_END);
        assert!(payload.is_empty());
    }

    #[test]
    fn read_frame_reports_clean_eof_as_none() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        assert!(read_frame(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn read_frame_errors_on_eof_mid_frame() {
        // Header promises 10 bytes, stream has 2.
        let mut data = vec![FRAME_OUTPUT, 0, 0, 0, 10];
        data.extend_from_slice(b"ab");
        let mut cursor = Cursor::new(data);
        let err = read_frame(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);

        // Truncated header.
        let mut cursor = Cursor::new(vec![FRAME_OUTPUT, 0]);
        let err = read_frame(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn read_frame_rejects_oversize_length() {
        let len = u32::try_from(MAX_FRAME_LEN + 1).unwrap();
        let mut data = vec![FRAME_OUTPUT];
        data.extend_from_slice(&len.to_be_bytes());
        let mut cursor = Cursor::new(data);
        let err = read_frame(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn read_frame_returns_unknown_types_for_caller_policy() {
        let frame = encode_frame(0x7f, b"future");
        let mut cursor = Cursor::new(frame);
        let (frame_type, payload) = read_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(frame_type, 0x7f);
        assert_eq!(payload, b"future");
    }

    #[test]
    fn subscribe_roundtrip() {
        for (want_input, token) in [(false, b"" as &[u8]), (true, b"secret-token" as &[u8])] {
            let frame = subscribe_frame(want_input, token);
            let mut cursor = Cursor::new(frame);
            let (frame_type, payload) = read_frame(&mut cursor).unwrap().unwrap();
            assert_eq!(frame_type, FRAME_SUBSCRIBE);
            let (got_want, got_token) = decode_subscribe(&payload).unwrap();
            assert_eq!(got_want, want_input);
            assert_eq!(got_token, token);
        }
    }

    #[test]
    fn decode_subscribe_rejects_malformed_payloads() {
        assert!(decode_subscribe(b"").is_none());
        assert!(decode_subscribe(&[0, 0]).is_none());
        // token_len says 5, only 3 bytes follow.
        assert!(decode_subscribe(&[1, 0, 5, b'a', b'b', b'c']).is_none());
        // trailing junk beyond token_len.
        assert!(decode_subscribe(&[1, 0, 1, b'a', b'b']).is_none());
    }

    #[test]
    fn hello_roundtrip() {
        for granted in [false, true] {
            let frame = hello_frame(granted);
            let mut cursor = Cursor::new(frame);
            let (frame_type, payload) = read_frame(&mut cursor).unwrap().unwrap();
            assert_eq!(frame_type, FRAME_HELLO);
            let (version, got) = decode_hello(&payload).unwrap();
            assert_eq!(version, PROTOCOL_VERSION);
            assert_eq!(got, granted);
        }
        assert!(decode_hello(&[1]).is_none());
    }

    #[test]
    fn resize_roundtrip() {
        let frame = resize_frame(42, 137);
        let mut cursor = Cursor::new(frame);
        let (frame_type, payload) = read_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(frame_type, FRAME_RESIZE);
        assert_eq!(decode_resize(&payload), Some((42, 137)));
        assert!(decode_resize(&[0, 0]).is_none());
        assert!(decode_resize(&[0, 0, 0, 0, 0]).is_none());
    }

    #[test]
    fn classify_greeting_requires_exact_magic() {
        assert_eq!(classify_greeting(b"APX1"), Greeting::V2);
        assert_eq!(classify_greeting(b""), Greeting::Legacy);
        assert_eq!(classify_greeting(b"APX"), Greeting::Legacy);
        assert_eq!(classify_greeting(b"APX2"), Greeting::Legacy);
        assert_eq!(classify_greeting(&[0, 0, 0, 0]), Greeting::Legacy);
        // The legacy blob starts with rows as u16 BE; a plausible first four
        // bytes of anything else must not match.
        assert_eq!(classify_greeting(b"apx1"), Greeting::Legacy);
    }

    #[test]
    fn snapshot_roundtrip_through_vt100() {
        use atuin_common::ansi::Vt100ParserExt as _;

        let mut parser = vt100::Parser::new_safe(4, 10, 0);
        parser.process(b"hi\r\nthere\x1b[1;3H");
        let snapshot = Snapshot::from_screen(parser.screen());
        assert_eq!((snapshot.rows, snapshot.cols), (4, 10));
        assert_eq!((snapshot.cursor_row, snapshot.cursor_col), (0, 2));
        assert_eq!(snapshot.rows_data.len(), 4);

        let decoded = Snapshot::decode(&snapshot.encode()).unwrap();
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn snapshot_decode_rejects_malformed_input() {
        assert!(Snapshot::decode(b"").is_none());
        assert!(Snapshot::decode(&[0; 7]).is_none());
        // Header + row length promising more bytes than exist.
        let mut data = vec![0, 2, 0, 5, 0, 0, 0, 0];
        data.extend_from_slice(&[0, 0, 0, 9]);
        data.extend_from_slice(b"ab");
        assert!(Snapshot::decode(&data).is_none());
    }

    #[test]
    fn snapshot_to_ansi_golden_bytes() {
        let snapshot = Snapshot {
            rows: 2,
            cols: 5,
            cursor_row: 1,
            cursor_col: 3,
            rows_data: vec![b"ab".to_vec(), b"cd".to_vec()],
        };
        let ansi = snapshot_to_ansi(&snapshot);
        assert_eq!(ansi, b"\x1b[H\x1b[2J\x1b[1;1Hab\x1b[2;1Hcd\x1b[0m\x1b[2;4H");
    }

    #[test]
    fn snapshot_encode_decode_to_ansi_golden_bytes() {
        // The full pipeline the tap client runs on a keyframe.
        let snapshot = Snapshot {
            rows: 1,
            cols: 4,
            cursor_row: 0,
            cursor_col: 0,
            rows_data: vec![b"\x1b[31mred".to_vec()],
        };
        let decoded = Snapshot::decode(&snapshot.encode()).unwrap();
        let ansi = snapshot_to_ansi(&decoded);
        assert_eq!(ansi, b"\x1b[H\x1b[2J\x1b[1;1H\x1b[31mred\x1b[0m\x1b[1;1H");
    }
}
