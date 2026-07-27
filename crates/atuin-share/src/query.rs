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

#[cfg(all(test, unix))]
mod tests {
    use super::replies;

    #[test]
    fn answers_cursor_position_report_1_indexed() {
        // vt100 reports 0-indexed; the CPR reply is 1-indexed.
        assert_eq!(replies(b"\x1b[6n", (4, 9)), b"\x1b[5;10R".to_vec());
    }

    #[test]
    fn answers_primary_device_attributes() {
        // Claim a plain VT102-ish terminal.
        assert_eq!(replies(b"\x1b[c", (0, 0)), b"\x1b[?6c".to_vec());
    }

    #[test]
    fn answers_device_attributes_with_explicit_zero_param() {
        assert_eq!(replies(b"\x1b[0c", (0, 0)), b"\x1b[?6c".to_vec());
    }

    #[test]
    fn ignores_ordinary_output() {
        assert!(replies(b"hello\r\n\x1b[1;31mred\x1b[0m", (0, 0)).is_empty());
    }

    #[test]
    fn answers_every_query_in_one_chunk_in_order() {
        assert_eq!(
            replies(b"a\x1b[6nb\x1b[cc", (0, 0)),
            b"\x1b[1;1R\x1b[?6c".to_vec()
        );
    }
}
