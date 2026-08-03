//! The host-side compositor: a persistent warning bar on row 1 and the child
//! shell repainted from our `vt100` model on the rows below it.

use std::borrow::Cow;
use std::io::Write as _;

use atuin_common::string::EllipsizeExt as _;
use atuin_common::string::Measure;
use atuin_common::string::align::Alignment;
use atuin_common::string::ellipsis::{Indicator, Pos};
use unicode_width::UnicodeWidthStr;

use crate::Size;

/// Whether viewers may type into the shared shell.
///
/// `--write` is a bool at the CLI boundary; inside the crate it travels as this
/// enum, so the flag can never be transposed with some other bool on its way
/// through the session, the renderer, and the transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteMode {
    ReadOnly,
    ReadWrite,
}

impl WriteMode {
    /// Convert the CLI's `--write` flag, once, at the crate boundary.
    #[must_use]
    pub(crate) fn from_flag(write: bool) -> Self {
        if write {
            Self::ReadWrite
        } else {
            Self::ReadOnly
        }
    }

    /// Whether viewer keystrokes may reach the child.
    #[must_use]
    pub(crate) fn is_write_enabled(self) -> bool {
        matches!(self, Self::ReadWrite)
    }

    /// The child's `ATUIN_SHARE_WRITE` environment value.
    #[must_use]
    pub(crate) fn as_env_value(self) -> &'static str {
        match self {
            Self::ReadWrite => "1",
            Self::ReadOnly => "0",
        }
    }
}

/// What the bar's chunks are joined with.
const SEPARATOR: &str = " | ";

/// Tier `0` segments are mandatory — the fact that the session is shared, and
/// whether viewers can type. They are never dropped voluntarily; on a terminal
/// too narrow even for them the text is hard-truncated instead.
const MANDATORY: u8 = 0;
/// The `Ctrl-\` hint: the last optional segment to survive as the bar narrows.
const TIER_HINT: u8 = 1;
/// The viewer count is dropped before the hint.
const TIER_VIEWERS: u8 = 2;
/// The explanatory prose is the first segment to go.
const TIER_PROSE: u8 = 3;

/// The warning bar's inputs: the viewer count and whether viewers can type.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StatusBar {
    pub(crate) viewers: u32,
    pub(crate) write: WriteMode,
    /// Viewer input died permanently for the rest of this process: the
    /// transport's never-forget replay ledger is full and every further
    /// viewer keystroke is refused (`transport::INPUT_NONCE_CAP`).
    ///
    /// Carried on the bar rather than announced once, because the condition is
    /// **terminal and silent**: the viewer is never told (that would need a
    /// wire change), so the host's terminal is the only place it can be seen,
    /// and a printed line lives only until the next repaint composites over
    /// the child area. A sticky segment is the durable form of a permanent
    /// state.
    pub(crate) input_disabled: bool,
}

impl StatusBar {
    /// Join every segment whose drop-tier is at most `tier`, in declaration
    /// order.
    ///
    /// Segments are dropped by *tier*, not by "whatever ran out of room", so a
    /// narrow bar degrades to a meaningful subset rather than to a sentence cut
    /// in half. The mandatory tier always survives this step.
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
    /// The bar is assembled from `|`-joined segments carrying a drop-tier. The
    /// richest tier that fits in `cols` wins, so the two things the host must
    /// always see — that the session is shared, and whether viewers can type —
    /// survive on a narrow terminal, while the explanatory prose and the viewer
    /// count fall away first. Simply ellipsizing one long fixed string would cut
    /// the write state off well before an 80-column terminal, which is precisely
    /// the information a host cannot afford to lose.
    ///
    /// The final fit is done in display columns via `atuin-common`'s
    /// `pad_ellipsize` (the same helper the search UI uses), which both
    /// truncates over-long text with `...` and pads short text with spaces.
    /// The bar text is pure ASCII today, but the fit stays column-based so a
    /// future wide glyph cannot overflow the row and wrap onto the child's
    /// first row.
    #[must_use]
    pub(crate) fn render(&self, cols: u16) -> Vec<u8> {
        let write_state = match (self.write.is_write_enabled(), self.input_disabled) {
            // Supersedes "WRITE ON": the share was started with `--write`, but
            // viewer typing is now refused for the rest of the process, so
            // claiming write is live would be a lie the host acts on. Stays in
            // the mandatory tier — a permanent state the host cannot afford to
            // lose is exactly what that tier is for.
            (true, true) => "INPUT DISABLED",
            (true, false) => "WRITE ON",
            // Unreachable in the read-only case: the transport refuses viewer
            // input before any AEAD work there, so a read-only share never
            // spends a unit of the budget that produces this state.
            (false, _) => "WRITE OFF",
        };
        let segments: [(u8, Cow<'static, str>); 5] = [
            (MANDATORY, Cow::Borrowed("! SHARED SESSION")),
            (TIER_PROSE, Cow::Borrowed("anything you type is visible")),
            (
                TIER_VIEWERS,
                Cow::Owned(format!("{} viewers", self.viewers)),
            ),
            (MANDATORY, Cow::Borrowed(write_state)),
            (TIER_HINT, Cow::Borrowed("Ctrl-\\ to end")),
        ];

        let mut tier = segments.iter().map(|(t, _)| *t).max().unwrap_or(MANDATORY);
        let text = loop {
            let candidate = Self::join_up_to_tier(&segments, tier);
            if tier == MANDATORY || UnicodeWidthStr::width(candidate.as_str()) <= cols as usize {
                break candidate;
            }
            tier -= 1;
        };

        let fitted = text.pad_ellipsize(
            Measure::Columns(cols as usize),
            Pos::End,
            Indicator::ASCII,
            Alignment::Start,
        );

        let mut out = Vec::new();
        out.extend_from_slice(b"\x1b[1;1H\x1b[7m"); // row 1, reverse video
        out.extend_from_slice(fitted.as_bytes());
        out.extend_from_slice(b"\x1b[0m"); // reset
        out
    }
}

/// Composites full host frames for one host-terminal geometry.
///
/// `avail` is the host's terminal **minus the bar row** — the rows the child may
/// occupy. That subtraction happens exactly once, in `run_share` (spec §6), and
/// `Session` tracks the already-subtracted value; building a `Compositor` from
/// the real terminal height would subtract the bar row a second time, leaving
/// the scroll region one row short of the child. That is not cosmetic: the
/// child's top line would scroll away on every repaint and the last physical row
/// would never be painted.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Compositor {
    pub(crate) avail: Size,
}

impl Compositor {
    /// Composite the full host frame: bar on row 1, the child screen on rows
    /// `2..=(1 + child.rows)`, then the hardware cursor at the child cursor
    /// position shifted down one row.
    ///
    /// The child screen is emitted inside a scroll region with origin mode ON,
    /// so `contents_formatted()`'s leading `\x1b[H` maps to physical row 2 and
    /// its following `\x1b[J` (ED-0, *erase forward only*) clears just the child
    /// area — the bar on row 1 is never touched. Because only *we* ever write to
    /// the real terminal (the child's output goes into the vt100 model, never
    /// straight to stdout) the child cannot clobber these modes.
    ///
    /// `child.rows` is clamped to `avail.rows`: the host can shrink their window
    /// between negotiations, and painting past the last visible row would scroll
    /// the whole screen and push the bar off it.
    #[must_use]
    pub(crate) fn composite(&self, screen: &vt100::Screen, child: Size, bar: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(bar); // positions itself absolutely at row 1

        // Bottom margin: the last row the child may occupy. Row 1 is the bar, so
        // the child's `n` rows end on physical row `1 + n`, never past the last
        // row available to it.
        let bottom = child.rows.min(self.avail.rows).saturating_add(1).max(2);

        // Scroll region rows 2..=bottom, origin mode ON.
        let _ = write!(&mut out, "\x1b[2;{bottom}r\x1b[?6h");
        out.extend_from_slice(&screen.contents_formatted());
        out.extend_from_slice(b"\x1b[?6l\x1b[r"); // origin off, reset scroll region

        // Final absolute cursor placement (origin mode now off).
        let (cr, cc) = screen.cursor_position();
        let _ = write!(&mut out, "\x1b[{};{}H", cr + 2, cc + 1);
        out
    }
}

/// The late-join / resync keyframe: a self-contained byte sequence that
/// repaints `screen` from a blank terminal, so a viewer joining mid-session
/// (or one whose replay buffer the hub dropped) can catch up in a single frame.
///
/// `vt100::Screen::contents_formatted` emits a clear followed by the full
/// visible contents with SGR state, so a fresh parser fed these bytes ends in
/// the same visible state.
#[must_use]
pub(crate) fn keyframe_bytes(screen: &vt100::Screen) -> Vec<u8> {
    screen.contents_formatted()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(cols: u16, viewers: u32, write: bool) -> Vec<u8> {
        StatusBar {
            viewers,
            write: WriteMode::from_flag(write),
            input_disabled: false,
        }
        .render(cols)
    }

    /// The same bar after viewer input died permanently (replay budget spent).
    fn exhausted_bar(cols: u16) -> Vec<u8> {
        StatusBar {
            viewers: 1,
            write: WriteMode::from_flag(true),
            input_disabled: true,
        }
        .render(cols)
    }

    /// Strip the bar's framing (`row 1, reverse video` … `reset`) and return
    /// the fitted text, asserting the framing bytes on the way.
    fn bar_text(out: &[u8]) -> String {
        let prefix = b"\x1b[1;1H\x1b[7m";
        let suffix = b"\x1b[0m";
        assert!(out.starts_with(prefix), "bar must home and reverse-video");
        assert!(out.ends_with(suffix), "bar must reset attributes");
        String::from_utf8(out[prefix.len()..out.len() - suffix.len()].to_vec())
            .expect("bar text is UTF-8")
    }

    #[test]
    fn write_mode_converts_the_flag_and_the_env_value() {
        assert_eq!(WriteMode::from_flag(true), WriteMode::ReadWrite);
        assert_eq!(WriteMode::from_flag(false), WriteMode::ReadOnly);
        assert!(WriteMode::ReadWrite.is_write_enabled());
        assert!(!WriteMode::ReadOnly.is_write_enabled());
        assert_eq!(WriteMode::ReadWrite.as_env_value(), "1");
        assert_eq!(WriteMode::ReadOnly.as_env_value(), "0");
    }

    #[test]
    fn full_bar_shows_all_segments_when_wide_enough() {
        let text = bar_text(&bar(100, 3, false));
        assert!(text.contains("! SHARED SESSION"));
        assert!(text.contains("anything you type is visible"));
        assert!(text.contains("3 viewers"));
        assert!(text.contains("WRITE OFF"));
        assert!(text.contains("Ctrl-\\ to end"));
        assert!(text.contains(SEPARATOR));
    }

    #[test]
    fn narrow_bar_drops_the_prose_first() {
        let text = bar_text(&bar(60, 3, false));
        assert!(!text.contains("anything you type is visible"));
        assert!(text.contains("3 viewers"));
        assert!(text.contains("Ctrl-\\ to end"));
        assert!(text.contains("SHARED SESSION"));
        assert!(text.contains("WRITE OFF"));
    }

    #[test]
    fn narrower_bar_drops_the_viewer_count_next() {
        let text = bar_text(&bar(50, 3, false));
        assert!(!text.contains("viewers"));
        assert!(text.contains("Ctrl-\\ to end"));
        assert!(text.contains("SHARED SESSION"));
        assert!(text.contains("WRITE OFF"));
    }

    #[test]
    fn mandatory_segments_survive_on_a_narrow_terminal() {
        let text = bar_text(&bar(30, 3, false));
        assert!(text.contains("SHARED SESSION"));
        assert!(text.contains("WRITE OFF"));
        assert!(!text.contains("viewers"));
        assert!(!text.contains("Ctrl-\\"));
        assert!(!text.contains("anything"));
    }

    #[test]
    fn below_mandatory_width_the_text_is_hard_truncated_with_ellipsis() {
        let text = bar_text(&bar(20, 0, false));
        assert!(text.starts_with('!'), "shared-session marker survives");
        assert!(text.ends_with("..."), "truncation is marked, not silent");
        assert_eq!(UnicodeWidthStr::width(text.as_str()), 20);
    }

    #[test]
    fn bar_reflects_the_write_state() {
        assert!(bar_text(&bar(100, 0, true)).contains("WRITE ON"));
        assert!(bar_text(&bar(100, 0, false)).contains("WRITE OFF"));
    }

    /// An exhausted input budget is a permanent, viewer-invisible condition,
    /// so the bar — the one host-facing surface a repaint cannot erase — must
    /// carry it, and must stop claiming the share still takes viewer input.
    /// It rides in the mandatory tier, so it survives a narrow terminal too.
    #[test]
    fn bar_shows_input_disabled_instead_of_write_on_when_the_budget_is_spent() {
        let wide = bar_text(&exhausted_bar(100));
        assert!(wide.contains("INPUT DISABLED"));
        assert!(!wide.contains("WRITE ON"));

        // Narrow enough that every optional tier is gone: the state is still
        // there, next to the mandatory shared-session marker.
        let narrow = bar_text(&exhausted_bar(35));
        assert!(narrow.contains("SHARED SESSION"));
        assert!(narrow.contains("INPUT DISABLED"));
        assert!(!narrow.contains("viewers"));
    }

    /// The fit is measured in **display columns**, not `char`s. The bar text
    /// is pure ASCII today, so the two agree — this pins the column-based fit
    /// so a future wide glyph cannot overflow the row and wrap onto the
    /// child's first row.
    #[test]
    fn bar_is_fitted_to_exactly_cols_display_columns() {
        for cols in [20u16, 30, 50, 60, 100] {
            let text = bar_text(&bar(cols, 3, false));
            assert_eq!(
                UnicodeWidthStr::width(text.as_str()),
                cols as usize,
                "bar must fill exactly {cols} columns"
            );
        }
    }

    fn parser_with(rows: u16, cols: u16, feed: &[u8]) -> vt100::Parser {
        let mut parser = vt100::Parser::new(rows, cols, 0);
        parser.process(feed);
        parser
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    #[test]
    fn composite_frames_the_child_in_a_scroll_region_below_the_bar() {
        let parser = parser_with(24, 80, b"hi");
        let size = Size { cols: 80, rows: 24 };
        let out = Compositor { avail: size }.composite(parser.screen(), size, b"BAR");

        // Bar first (it positions itself absolutely at row 1) …
        assert!(out.starts_with(b"BAR"));
        // … then scroll region rows 2..=25 with origin mode ON …
        assert!(out[3..].starts_with(b"\x1b[2;25r\x1b[?6h"));
        // … the repaint itself …
        assert!(find(&out, &parser.screen().contents_formatted()).is_some());
        // … then origin off, region reset, and the cursor at the child's (0, 2)
        // shifted down one row for the bar and converted to 1-indexing.
        assert!(out.ends_with(b"\x1b[?6l\x1b[r\x1b[2;3H"));
    }

    #[test]
    fn composite_places_the_cursor_below_the_bar_one_indexed() {
        let parser = parser_with(24, 80, b"a\r\nbc");
        let size = Size { cols: 80, rows: 24 };
        let out = Compositor { avail: size }.composite(parser.screen(), size, b"");
        // Child cursor (1, 2), 0-indexed → physical row 3, column 3.
        assert!(out.ends_with(b"\x1b[3;3H"));
    }

    /// Regression guard for the double-subtraction bug: `avail` is already the
    /// terminal height *minus the bar row*, so the region bottom must be
    /// `min(child, avail) + 1` — subtracting the bar row again here would leave
    /// the region one row short and scroll the child's top line away on every
    /// repaint.
    #[test]
    fn composite_clamps_the_region_to_avail_when_the_host_shrank() {
        let parser = parser_with(24, 80, b"");
        let child = Size { cols: 80, rows: 24 };
        let avail = Size { cols: 80, rows: 10 };
        let out = Compositor { avail }.composite(parser.screen(), child, b"");
        assert!(find(&out, b"\x1b[2;11r\x1b[?6h").is_some());
    }

    #[test]
    fn composite_uses_child_rows_when_the_child_is_smaller_than_avail() {
        let parser = parser_with(5, 80, b"");
        let child = Size { cols: 80, rows: 5 };
        let avail = Size { cols: 80, rows: 24 };
        let out = Compositor { avail }.composite(parser.screen(), child, b"");
        assert!(find(&out, b"\x1b[2;6r\x1b[?6h").is_some());
    }

    #[test]
    fn composite_region_bottom_never_rises_above_row_2() {
        let parser = parser_with(1, 80, b"");
        let child = Size { cols: 80, rows: 0 };
        let avail = Size { cols: 80, rows: 1 };
        let out = Compositor { avail }.composite(parser.screen(), child, b"");
        assert!(find(&out, b"\x1b[2;2r\x1b[?6h").is_some());
    }

    /// The keyframe contract: a *fresh* parser fed the keyframe ends in the
    /// same visible state as the live one — text, cursor, and SGR state.
    #[test]
    fn keyframe_replayed_on_a_blank_terminal_reproduces_the_screen() {
        let mut live = vt100::Parser::new(10, 40, 0);
        live.process(b"hello\r\nworld \x1b[31mred\x1b[0m plain\r\n\x1b[3;7Hcursor");

        let keyframe = keyframe_bytes(live.screen());

        let mut joined = vt100::Parser::new(10, 40, 0);
        joined.process(&keyframe);

        assert_eq!(joined.screen().contents(), live.screen().contents());
        assert_eq!(
            joined.screen().cursor_position(),
            live.screen().cursor_position()
        );
        assert_eq!(
            joined.screen().contents_formatted(),
            live.screen().contents_formatted()
        );
    }
}
