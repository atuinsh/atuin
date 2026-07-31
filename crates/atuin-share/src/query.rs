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
pub(crate) fn replies(chunk: &[u8], cursor: (u16, u16)) -> Vec<u8> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpr_reply_is_one_indexed() {
        // vt100 reports (0, 0) for the home position; CPR speaks 1-indexed.
        assert_eq!(replies(b"\x1b[6n", (0, 0)), b"\x1b[1;1R");
        assert_eq!(replies(b"\x1b[6n", (4, 9)), b"\x1b[5;10R");
    }

    #[test]
    fn device_attributes_answered_for_bare_and_zero_param_forms() {
        assert_eq!(replies(b"\x1b[c", (0, 0)), b"\x1b[?6c");
        assert_eq!(replies(b"\x1b[0c", (0, 0)), b"\x1b[?6c");
    }

    #[test]
    fn other_csi_sequences_are_ignored() {
        // ED, SGR (including a "6"-parameter one), and a DA form we don't
        // answer (params other than "" / "0").
        assert_eq!(replies(b"\x1b[2J", (0, 0)), b"");
        assert_eq!(replies(b"\x1b[6m", (0, 0)), b"");
        assert_eq!(replies(b"\x1b[38;5;1m", (0, 0)), b"");
        assert_eq!(replies(b"\x1b[1c", (0, 0)), b"");
    }

    #[test]
    fn non_csi_escapes_are_ignored() {
        assert_eq!(replies(b"\x1b(B\x1b=", (0, 0)), b"");
    }

    #[test]
    fn probe_split_at_the_chunk_boundary_is_dropped() {
        // Documented behavior: no reassembly across chunks — a truncated probe
        // gets no reply at all, from either fragment.
        assert_eq!(replies(b"\x1b", (0, 0)), b"");
        assert_eq!(replies(b"\x1b[", (0, 0)), b"");
        assert_eq!(replies(b"\x1b[6", (0, 0)), b"");
        assert_eq!(replies(b"n", (0, 0)), b"");
    }

    #[test]
    fn multiple_probes_in_one_chunk_are_all_answered_in_order() {
        assert_eq!(
            replies(b"\x1b[6n\x1b[0c", (2, 3)),
            b"\x1b[3;4R\x1b[?6c".as_slice()
        );
    }

    #[test]
    fn probes_embedded_in_ordinary_output_are_found() {
        assert_eq!(replies(b"hello\x1b[6nworld", (0, 0)), b"\x1b[1;1R");
    }
}
