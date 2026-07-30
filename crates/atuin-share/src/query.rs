//! Synthetic answers to the terminal queries the compositor would swallow.

/// Scan a chunk of child output for terminal queries that the compositor would
/// otherwise swallow, and produce the replies to write back into the child's
/// PTY.
///
/// Only the two probes that commonly gate startup are handled:
///   * `ESC [ 6 n`         Cursor Position Report  -> `ESC [ <row> ; <col> R`
///   * `ESC [ c` / `ESC [ 0 c`  Primary Device Attributes -> `ESC [ ? 6 c`
///
/// `cursor` is the child's cursor as reported by `vt100` (0-indexed); CPR is
/// 1-indexed, hence the `+ 1`. Mouse reporting and graphics protocols are out
/// of scope (see the crate-level "Known limitations").
///
/// Sequences split across chunk boundaries are not reassembled: a probe is at
/// most 4 bytes and PTY reads deliver it whole in practice. A missed probe
/// degrades exactly as today (no reply), so this stays a pure function.
#[must_use]
pub fn replies(chunk: &[u8], cursor: (u16, u16)) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(esc) = chunk[i..].iter().position(|&b| b == 0x1b) {
        let start = i + esc;
        // Need at least "ESC [ x"
        if start + 2 >= chunk.len() {
            break;
        }
        if chunk[start + 1] != b'[' {
            i = start + 1;
            continue;
        }
        // Collect the numeric parameter bytes after "ESC [".
        let mut j = start + 2;
        while j < chunk.len() && chunk[j].is_ascii_digit() {
            j += 1;
        }
        if j >= chunk.len() {
            break;
        }
        let params = &chunk[start + 2..j];
        match chunk[j] {
            b'n' if params == b"6" => {
                let (row, col) = cursor;
                out.extend_from_slice(
                    format!("\x1b[{};{}R", row.saturating_add(1), col.saturating_add(1)).as_bytes(),
                );
            }
            b'c' if params.is_empty() || params == b"0" => {
                out.extend_from_slice(b"\x1b[?6c");
            }
            _ => {}
        }
        i = j + 1;
    }
    out
}
