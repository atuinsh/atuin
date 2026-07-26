// The bar and compositor are a complete, self-contained unit exercised by their
// own tests, but their only in-crate consumer is the `session` module, which
// lands in a later task of this plan. Until then these are technically dead code
// and `cargo clippy -- -D warnings` (what CI runs) would reject the crate.
#![allow(dead_code)]

//! The host-side compositor: a persistent warning bar on row 1 and the child
//! shell repainted from our `vt100` model on the rows below it.

use std::borrow::Cow;
use std::fmt::Write as _;

use atuin_common::string::EllipsizeExt as _;
use atuin_common::string::Measure;
use atuin_common::string::align::Alignment;
use atuin_common::string::ellipsis::{Indicator, Pos};
use unicode_width::UnicodeWidthStr;

use crate::Size;

/// Display width of the printable columns, ignoring ANSI escape sequences.
/// Test + assertion helper. Measured in *columns*, not chars: the bar text
/// contains wide glyphs (`⚠`) and a char count would under-measure them.
pub(crate) fn visible_width(s: &str) -> usize {
    let mut visible = String::with_capacity(s.len());
    let mut in_esc = false;
    for ch in s.chars() {
        if in_esc {
            if ch.is_ascii_alphabetic() {
                in_esc = false;
            }
        } else if ch == '\x1b' {
            in_esc = true;
        } else {
            visible.push(ch);
        }
    }
    UnicodeWidthStr::width(visible.as_str())
}

/// What the bar's chunks are joined with.
const SEPARATOR: &str = " · ";

/// Tier `0` segments are mandatory — the fact that the session is shared, and
/// whether viewers can type. They are never dropped voluntarily; on a terminal
/// too narrow even for them the text is hard-truncated instead.
const MANDATORY: u8 = 0;

/// Join every segment whose drop-tier is at most `tier`, in declaration order.
///
/// Segments are dropped by *tier*, not by "whatever ran out of room", so a
/// narrow bar degrades to a meaningful subset rather than to a sentence cut in
/// half. The mandatory tier always survives this step.
fn join_up_to_tier(segments: &[(u8, Cow<'static, str>)], tier: u8) -> String {
    let mut out = String::new();
    for (_, text) in segments.iter().filter(|(t, _)| *t <= tier) {
        if !out.is_empty() {
            out.push_str(SEPARATOR);
        }
        out.push_str(text);
    }
    out
}

/// Render the warning bar as a full row: reverse-video, fitted to exactly
/// `cols` display columns.
///
/// The bar is assembled from `·`-joined segments carrying a drop-tier. The
/// richest tier that fits in `cols` wins, so the two things the host must
/// always see — that the session is shared, and whether viewers can type —
/// survive on a narrow terminal, while the explanatory prose and the viewer
/// count fall away first. Simply ellipsizing one long fixed string would cut
/// the write state off well before an 80-column terminal, which is precisely
/// the information a host cannot afford to lose.
///
/// The final fit is done in display columns via `atuin-common`'s
/// `pad_ellipsize` (the same helper the search UI uses), which both truncates
/// over-long text with `…` and pads short text with spaces. A naive
/// `chars().take(cols)` would overflow the row whenever a wide glyph is
/// present — the text opens with `⚠`, which some terminals render 2 columns
/// wide — and the overflow would wrap onto the child's first row.
#[must_use]
pub fn render_bar(cols: u16, viewers: u32, write: bool) -> Vec<u8> {
    let write_state = if write { "WRITE ON" } else { "WRITE OFF" };
    let segments: [(u8, Cow<'static, str>); 5] = [
        (MANDATORY, Cow::Borrowed("⚠ SHARED SESSION")),
        (3, Cow::Borrowed("anything you type is visible")),
        (2, Cow::Owned(format!("{viewers} viewers"))),
        (MANDATORY, Cow::Borrowed(write_state)),
        (1, Cow::Borrowed("Ctrl-\\ to end")),
    ];

    let mut tier = segments.iter().map(|(t, _)| *t).max().unwrap_or(MANDATORY);
    let text = loop {
        let candidate = join_up_to_tier(&segments, tier);
        if tier == MANDATORY || UnicodeWidthStr::width(candidate.as_str()) <= cols as usize {
            break candidate;
        }
        tier -= 1;
    };

    let fitted = text.pad_ellipsize(
        Measure::Columns(cols as usize),
        Pos::End,
        Indicator::UNICODE,
        Alignment::Start,
    );

    let mut out = Vec::new();
    out.extend_from_slice(b"\x1b[1;1H\x1b[7m"); // row 1, reverse video
    out.extend_from_slice(fitted.as_bytes());
    out.extend_from_slice(b"\x1b[0m"); // reset
    out
}

/// Composite the full host frame: bar on row 1, the child screen on rows
/// `2..=(1 + child.rows)`, then the hardware cursor at the child cursor
/// position shifted down one row.
///
/// The child screen is emitted inside a scroll region with origin mode ON, so
/// `contents_formatted()`'s leading `\x1b[H` maps to physical row 2 and its
/// following `\x1b[J` (ED-0, *erase forward only*) clears just the child area —
/// the bar on row 1 is never touched. Because only *we* ever write to the real
/// terminal (the child's output goes into the vt100 model, never straight to
/// stdout) the child cannot clobber these modes.
///
/// `child.rows` is clamped to the rows physically available below the bar: the
/// host can shrink their window between negotiations, and painting past the
/// last visible row would scroll the whole screen and push the bar off it.
#[must_use]
pub fn composite(screen: &vt100::Screen, child: Size, physical: Size, bar: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(bar); // positions itself absolutely at row 1

    // Bottom margin: last row the child may occupy, never past the screen.
    let bottom = (1 + child.rows).min(physical.rows).max(2);

    // Scroll region rows 2..=bottom, origin mode ON.
    let _ = write!(&mut Utf8(&mut out), "\x1b[2;{bottom}r\x1b[?6h");
    out.extend_from_slice(&screen.contents_formatted());
    out.extend_from_slice(b"\x1b[?6l\x1b[r"); // origin off, reset scroll region

    // Final absolute cursor placement (origin mode now off).
    let (cr, cc) = screen.cursor_position();
    let _ = write!(&mut Utf8(&mut out), "\x1b[{};{}H", cr + 2, cc + 1);
    out
}

/// Tiny adapter so `write!` can target a `Vec<u8>`.
struct Utf8<'a>(&'a mut Vec<u8>);
impl std::fmt::Write for Utf8<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn bar_is_clamped_to_width_and_mentions_state() {
        let bar = render_bar(30, 3, true);
        let text = String::from_utf8_lossy(&bar);
        assert!(text.contains("SHARED SESSION"));
        assert!(text.contains("WRITE ON"));
        // The visible run must occupy exactly `cols` display columns: the bar is
        // a full reverse-video row, so it is padded as well as truncated.
        assert_eq!(super::visible_width(&text), 30);
    }

    #[test]
    fn bar_shows_write_off_when_read_only() {
        let text = String::from_utf8_lossy(&render_bar(80, 0, false)).into_owned();
        assert!(text.contains("WRITE OFF"));
        assert_eq!(super::visible_width(&text), 80);
    }

    #[test]
    fn bar_width_is_display_columns_not_char_count() {
        // The bar text opens with '⚠', which is a wide (2-column) glyph. A
        // char-count clamp would overflow the row and wrap, destroying the
        // layout, so the budget must be measured in display columns.
        for cols in [1u16, 2, 3, 8, 40] {
            assert_eq!(
                super::visible_width(&String::from_utf8_lossy(&render_bar(cols, 1, true))),
                cols as usize,
                "bar must be exactly {cols} columns wide"
            );
        }
    }

    #[test]
    fn composite_places_bar_on_row1_and_child_below() {
        let mut p = vt100::Parser::new(4, 20, 0); // child area: 4 rows
        p.process(b"\x1b[HABC");
        let child = crate::Size { cols: 20, rows: 4 };
        let physical = crate::Size { cols: 20, rows: 5 }; // 1 bar row + 4 child rows
        let bar = render_bar(20, 1, false);
        let frame = composite(p.screen(), child, physical, &bar);
        let s = String::from_utf8_lossy(&frame);

        // The bar is written first, anchored absolutely at row 1.
        assert!(s.starts_with("\x1b[1;1H"));
        // Scroll region spans the child's rows only: physical row 2 ..= 1+child.rows.
        assert!(
            s.contains("\x1b[2;5r"),
            "scroll region must be 2..=5, got {s:?}"
        );
        // Origin mode is enabled for the child repaint, then disabled again.
        assert!(s.contains("\x1b[?6h"));
        assert!(s.contains("\x1b[?6l"));
        // The cursor is finally placed at child cursor row+2 / col+1, absolute.
        let (cr, cc) = p.screen().cursor_position();
        assert!(s.contains(&format!("\x1b[{};{}H", cr + 2, cc + 1)));
    }

    #[test]
    fn composite_clamps_child_taller_than_the_visible_area() {
        // If the host shrinks their window, `physical` can be smaller than the
        // last negotiated `child`. Painting `child.rows` regardless would run
        // past the last visible row, scroll the screen, and destroy the bar.
        let mut p = vt100::Parser::new(20, 20, 0);
        p.process(b"\x1b[Hx");
        let child = crate::Size { cols: 20, rows: 20 };
        let physical = crate::Size { cols: 20, rows: 6 }; // only 5 rows for the child
        let bar = render_bar(20, 0, false);
        let s = String::from_utf8_lossy(&composite(p.screen(), child, physical, &bar)).into_owned();
        // Bottom margin is clamped to the physical last row, never beyond it.
        assert!(
            s.contains("\x1b[2;6r"),
            "expected clamped region 2..=6, got {s:?}"
        );
    }
}
