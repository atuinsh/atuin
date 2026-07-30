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
/// `avail` is the host's terminal **minus the bar row** — the rows the child may
/// occupy. That subtraction happens exactly once, in `run_share` (spec §6), and
/// `Session` tracks the already-subtracted value; passing the real terminal
/// height here would subtract the bar row a second time, leaving the scroll
/// region one row short of the child. That is not cosmetic: the child's top line
/// would scroll away on every repaint and the last physical row would never be
/// painted.
///
/// `child.rows` is clamped to `avail.rows`: the host can shrink their window
/// between negotiations, and painting past the last visible row would scroll the
/// whole screen and push the bar off it.
#[must_use]
pub fn composite(screen: &vt100::Screen, child: Size, avail: Size, bar: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(bar); // positions itself absolutely at row 1

    // Bottom margin: the last row the child may occupy. Row 1 is the bar, so the
    // child's `n` rows end on physical row `1 + n`, never past the last row
    // available to it.
    let bottom = child.rows.min(avail.rows).saturating_add(1).max(2);

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
