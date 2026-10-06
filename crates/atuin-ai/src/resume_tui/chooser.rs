//! The "Resume in" chooser: once a session is chosen (enter, or tab to edit the command), where
//! to resume it.
//!
//! Its own harness comes first and is preselected, so enter-enter resumes it; a session whose
//! transcript isn't on this machine is restored from sync behind the scenes, which the line only
//! hints at. Right under it, when its harness is installed and atuin can write its sessions,
//! forking it: a new session of the same harness, with the same history (see
//! [`atuin_common::harnesstools::fork`]); `f` selects it, and it is preselected when the original
//! can't resume here or an agent here has it open. It is dimmed, and can't be picked, once
//! the session turns out to hold no conversation to fork. Then every other harness installed
//! here, to continue the session in: it is written out as a new session there, its tool calls
//! flattened into notes, and that is resumed. Harnesses
//! that aren't installed aren't listed: there is nothing to do with them. The session's own
//! harness is always listed, dimmed with the reason when it can't resume it here (a Copilot
//! session, a directory that's gone, a harness that isn't installed), and the first line that
//! works is preselected instead.
//!
//! When catching the session's copy here up with sync needs a choice ([`Held`]), the chooser opens
//! again on it, saying why: its first line resumes the copy here as it is (and stays preselected),
//! then a switch line for each head the copy can be switched to (written out again along that
//! head's branch, in place; see [`super::catchup`]), and a fork line for each of the session's
//! heads, newest first, forks from that head; the newest fork is preselected when an agent here
//! has the session open (and no switch line is offered then).
//!
//! The chooser keeps the key's meaning: opened with enter it resumes (or edits, without
//! `enter_accept`), opened with tab it edits, and tab in it always edits.

use atuin_client::ai_session::{HarnessKind, HarnessSession};
use atuin_client::settings::Settings;
use atuin_client::theme::{Meaning, Theme};
use atuin_client::tui::key::{KeyCodeValue, SingleKey};
use atuin_common::harnesstools::continuation::NothingToContinue;
use atuin_common::string::ellipsis::{Indicator, Pos};
use atuin_common::string::{EllipsizeExt as _, Measure};
use atuin_common::time::OffsetDateTimeExt as _;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use time::OffsetDateTime;
use unicode_width::UnicodeWidthStr;

use super::catchup::{Branch, Held, Why};
use super::render::{harness_style, style};
use super::resumer::{NotResumable, ResumePlan};
use super::source::{SessionRow, harness_badge, harness_label};
use super::state::{InputAction, LIVE_SECS, Pending, Picked, State};

/// The chooser, while it's open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chooser {
    /// The session it opened on: what a pick acts on, whatever the list has moved to since.
    pub row: SessionRow,
    /// The other harnesses installed here, to continue the session in: the lines after the
    /// original's and the fork's.
    pub targets: Vec<HarnessKind>,
    /// Whether the session can be forked here: line 2 forks it.
    pub fork: bool,
    /// The selected line, from 0.
    pub selected: usize,
    /// What picking a line with enter (or its digit) does: what the key that opened it asked for.
    pub action: Pending,
    /// Whether the selection was moved by hand: if not, it moves off the first line once that
    /// turns out not to work.
    pub moved: bool,
    /// Set when it opened on catching up needing a choice: the first line resumes the copy here
    /// as it is, and the fork lines fork from the session's heads.
    pub held: Option<Box<Held>>,
}

/// What a line of the chooser does with the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// Resume it in its own harness.
    Original,
    /// Resume the copy here as it is, with this plan, rather than catch it up.
    AsIs(Box<ResumePlan>),
    /// Fork it: a new session of its own harness, with the same history (that of a head, when
    /// given).
    Fork(Option<Box<Branch>>),
    /// Switch the copy here to a head's branch (written out again along it, in place), and
    /// resume it.
    Switch(Box<Branch>),
    /// Continue it in another harness.
    Continue(HarnessKind),
}

impl Chooser {
    /// How many fork lines it has.
    fn forks(&self) -> usize {
        match &self.held {
            Some(held) if self.fork => held.branches.len(),
            _ => usize::from(self.fork),
        }
    }

    /// How many switch lines it has: one per head the copy here can be switched to.
    fn switches(&self) -> usize {
        self.held.as_ref().map_or(0, |held| held.switches().count())
    }

    /// The first fork line.
    pub fn first_fork(&self) -> usize {
        1 + self.switches()
    }

    pub fn len(&self) -> usize {
        1 + self.switches() + self.forks() + self.targets.len()
    }

    /// What line `n` does.
    pub fn line(&self, n: usize) -> Destination {
        let forks = self.first_fork();
        let first = forks + self.forks();
        match (n, &self.held) {
            (0, None) => Destination::Original,
            (0, Some(held)) => Destination::AsIs(Box::new(held.plan.clone())),
            (n, Some(held)) if n < forks => held
                .switches()
                .nth(n - 1)
                .cloned()
                .map_or(Destination::Original, |b| Destination::Switch(Box::new(b))),
            (n, None) if n < first => Destination::Fork(None),
            (n, Some(held)) if n < first => {
                Destination::Fork(held.branches.get(n - forks).cloned().map(Box::new))
            }
            (n, _) => self
                .targets
                .get(n - first)
                .map_or(Destination::Original, |t| Destination::Continue(*t)),
        }
    }

    /// Select line `n`.
    fn select(&mut self, n: usize) {
        self.selected = n;
        self.moved = true;
    }
}

/// One line of the chooser, as drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub harness: HarnessKind,
    /// `original`, `continue, 42 tool calls become notes`, …
    pub detail: String,
    /// Why it can't be picked, if it can't.
    pub unavailable: Option<String>,
}

/// Whether the session itself looks to be running still: written to in the last
/// [`LIVE_SECS`]. Only its own messages count, not its children's (a fork or subagent busy
/// under an idle session leaves the session free to resume).
fn running(now: OffsetDateTime, row: &SessionRow) -> bool {
    now.saturating_duration_since(row.active_at).as_secs() < LIVE_SECS
}

/// What resuming a session that is still running (see [`running`]) does, briefly. Claude Code
/// turns a `--resume` of a running session into a fork (a new session id, grouped under the
/// original); the others open the same session again, and both processes append to it.
fn concurrent(harness: HarnessKind) -> &'static str {
    match harness {
        HarnessKind::ClaudeCode => "resuming forks it",
        _ => "both will append to it",
    }
}

/// Why a session can't resume in its own harness, briefly.
fn short_reason(why: &NotResumable) -> String {
    match why {
        NotResumable::NotInstalled(program) => format!("`{program}` isn't installed"),
        why => why.to_string(),
    }
}

impl State {
    /// Open the chooser on `row` (the session acted on), offering its own harness, forking it
    /// (with `fork`) and `targets` (the other harnesses installed here); picking a line with
    /// enter does `action`. The first line that works is selected.
    pub fn open_chooser(
        &mut self,
        row: &SessionRow,
        targets: Vec<HarnessKind>,
        fork: bool,
        action: Pending,
    ) {
        self.chooser = Some(Chooser {
            row: row.clone(),
            targets,
            fork,
            selected: 0,
            action,
            moved: false,
            held: None,
        });
        self.settle_chooser();
    }

    /// Open the chooser on `row` for the choice catching it up needs (`held`): resume the copy
    /// here as it is, fork from a head (with `fork`), or continue it in one of `targets`. The
    /// status line says why.
    pub fn open_choice(
        &mut self,
        row: &SessionRow,
        held: Box<Held>,
        targets: Vec<HarnessKind>,
        fork: bool,
        action: Pending,
    ) {
        self.status = Some((held.status(), Meaning::AlertWarn));
        self.chooser = Some(Chooser {
            row: row.clone(),
            targets,
            fork: fork && !held.branches.is_empty(),
            selected: 0,
            action,
            moved: false,
            held: Some(held),
        });
        self.settle_chooser();
    }

    /// Whether the session's own harness can't resume it here, and why (once its plan is known).
    pub fn original_unavailable(&self, session: &HarnessSession) -> Option<&NotResumable> {
        self.plans.get(session)?.as_ref().err()
    }

    /// Whether the session holds no conversation to fork, as reading what continuing it would
    /// flatten found (once that's known): forking it would fail.
    pub fn nothing_to_fork(&self, session: &HarnessSession) -> bool {
        matches!(self.flattened.get(session), Some(Err(why)) if *why == NothingToContinue.to_string())
    }

    /// Move an untouched selection off the first line once its plan says it can't be picked,
    /// or onto the fork when an agent here has the session open ([`Resume::live`]), which
    /// resuming it again would race. Never onto a fork with nothing to fork: off it again, if it
    /// was moved there already. For a choice catching up needs, the copy here as it is, or the
    /// fork from the newest head while an agent here has the session open.
    ///
    /// [`Resume::live`]: super::resumer::Resume::live
    pub fn settle_chooser(&mut self) {
        let Some(chooser) = &self.chooser else {
            return;
        };
        if chooser.moved || chooser.len() < 2 {
            return;
        }
        let row = &chooser.row;
        let fork = chooser.fork && !self.nothing_to_fork(&row.handle);
        if let Some(held) = &chooser.held {
            let selected = if fork && held.why == Why::Live {
                chooser.first_fork()
            } else {
                0
            };
            if let Some(chooser) = self.chooser.as_mut() {
                chooser.selected = selected;
            }
            return;
        }
        let plan = self.plans.get(&row.handle).and_then(|p| p.as_ref().ok());
        let running = fork && plan.is_some_and(|p| p.live);
        let selected = if !running && self.original_unavailable(&row.handle).is_none() {
            0
        } else if fork || !chooser.fork {
            1
        } else if chooser.len() > 2 {
            // The first harness to continue it in, past the fork that can't be picked.
            2
        } else {
            0
        };
        if let Some(chooser) = self.chooser.as_mut() {
            chooser.selected = selected;
        }
    }

    /// Select the chooser's fork line, as if by hand.
    pub fn select_fork(&mut self) {
        if let Some(chooser) = self.chooser.as_mut().filter(|c| c.fork) {
            chooser.select(chooser.first_fork());
        }
    }

    /// The chooser's lines.
    pub fn choices(&self) -> Vec<Choice> {
        let Some(chooser) = &self.chooser else {
            return Vec::new();
        };
        let session = &chooser.row.handle;
        // As the list last read it, while it still shows it.
        let row = self.target().filter(|r| r.handle == *session).unwrap_or(&chooser.row);
        let live = running((self.now)(), row);
        if let Some(held) = &chooser.held {
            return self.held_choices(chooser, held);
        }
        let original = match self.plans.get(session) {
            Some(Err(why)) => Choice {
                harness: session.harness,
                detail: "original".to_owned(),
                unavailable: Some(short_reason(why)),
            },
            Some(Ok(resume)) if resume.restore.is_some() => Choice {
                harness: session.harness,
                detail: if live {
                    "original, from sync · still running — this resumes a copy".to_owned()
                } else {
                    "original, from sync".to_owned()
                },
                unavailable: None,
            },
            _ => Choice {
                harness: session.harness,
                detail: if live {
                    format!("original · running elsewhere — {}", concurrent(session.harness))
                } else {
                    "original".to_owned()
                },
                unavailable: None,
            },
        };
        let flattened = match self.flattened.get(session) {
            Some(Ok(flattened)) => flattened.summary(),
            _ => String::new(),
        };
        let fork = chooser.fork.then(|| {
            if self.nothing_to_fork(session) {
                Choice {
                    harness: session.harness,
                    detail: "fork".to_owned(),
                    unavailable: Some("the session has no messages".to_owned()),
                }
            } else {
                Choice {
                    harness: session.harness,
                    detail: "fork: new session, same history".to_owned(),
                    unavailable: None,
                }
            }
        });
        let continued = chooser.targets.iter().map(|target| Choice {
            harness: *target,
            detail: if flattened.is_empty() {
                "continue".to_owned()
            } else {
                format!("continue, {flattened}")
            },
            unavailable: None,
        });
        std::iter::once(original).chain(fork).chain(continued).collect()
    }

    /// The chooser's lines for a choice catching up needs.
    fn held_choices(&self, chooser: &Chooser, held: &Held) -> Vec<Choice> {
        let harness = chooser.row.handle.harness;
        let line = |detail: String| Choice {
            harness,
            detail,
            unavailable: None,
        };
        let detail = match held.why {
            Why::Live => "this copy as is · running here",
            _ => "this copy as is",
        };
        let now = (self.now)();
        let switches = held.switches().map(|b| line(b.switch_line()));
        let forks = held.branches.iter().take(chooser.forks()).map(|b| line(b.line(now)));
        let continued = chooser.targets.iter().map(|target| Choice {
            harness: *target,
            detail: "continue".to_owned(),
            unavailable: None,
        });
        std::iter::once(line(detail.to_owned()))
            .chain(switches)
            .chain(forks)
            .chain(continued)
            .collect()
    }

    /// A key while the chooser is open: move (up/down, ctrl-p/ctrl-n, k/j, f to the fork), pick
    /// (enter does what opened the chooser, tab edits, ctrl-y copies the command, a digit picks
    /// its line), or go back to the list (esc, q, ctrl-c, ctrl-g).
    pub fn chooser_key(&mut self, key: &SingleKey) -> InputAction {
        let Some(chooser) = self.chooser.as_mut() else {
            return InputAction::Continue;
        };
        let last = chooser.len() - 1;
        let pick = match (&key.code, key.ctrl) {
            (KeyCodeValue::Esc, _)
            | (KeyCodeValue::Char('c' | 'g' | '['), true)
            | (KeyCodeValue::Char('q'), false) => {
                self.chooser = None;
                return InputAction::Continue;
            }
            (KeyCodeValue::Up, _)
            | (KeyCodeValue::Char('p'), true)
            | (KeyCodeValue::Char('k'), false) => {
                chooser.select(chooser.selected.saturating_sub(1));
                return InputAction::Continue;
            }
            (KeyCodeValue::Down, _)
            | (KeyCodeValue::Char('n'), true)
            | (KeyCodeValue::Char('j'), false) => {
                chooser.select((chooser.selected + 1).min(last));
                return InputAction::Continue;
            }
            (KeyCodeValue::Char('f'), false) => {
                self.select_fork();
                return InputAction::Continue;
            }
            (KeyCodeValue::Char(c @ '1'..='9'), false) => {
                let n = c.to_digit(10).and_then(|n| usize::try_from(n).ok()).unwrap_or(0);
                if n > chooser.len() {
                    return InputAction::Continue;
                }
                chooser.select(n - 1);
                chooser.action
            }
            (KeyCodeValue::Enter, _) | (KeyCodeValue::Char('m'), true) => chooser.action,
            (KeyCodeValue::Tab, _) => Pending::Edit,
            (KeyCodeValue::Char('y'), true) => Pending::Copy,
            _ => return InputAction::Continue,
        };
        let line = chooser.line(chooser.selected);
        let row = chooser.row.clone();
        if line == Destination::Original
            && pick != Pending::Copy
            && self.original_unavailable(&row.handle).is_some()
        {
            // Its line already says why.
            let label = harness_label(row.handle.harness);
            self.status = Some((
                format!("{label} can't resume it here: pick another line"),
                Meaning::AlertError,
            ));
            return InputAction::Continue;
        }
        if matches!(line, Destination::Fork(_)) && self.nothing_to_fork(&row.handle) {
            // Its line already says why; the `--fork` command copied would fail as well.
            self.status =
                Some(("nothing to fork: pick another line".to_owned(), Meaning::AlertError));
            return InputAction::Continue;
        }
        if pick != Pending::Copy {
            self.chooser = None;
        }
        InputAction::Pick(Box::new(Picked {
            row,
            line,
            action: pick,
        }))
    }

    /// The chooser, over the list against the selected row (above it, or below it when
    /// inverted), its badges under the list's. Centred when the list isn't showing.
    pub fn draw_chooser(&self, f: &mut Frame, settings: &Settings, theme: &Theme) {
        let Some(chooser) = &self.chooser else {
            return;
        };
        let area = f.area();
        let anchor = self.list_anchor;
        // Inside the list, less the popup's borders and padding.
        let room = anchor.map_or(area.width, |a| a.list.width);
        let budget = usize::from(room.saturating_sub(4));
        let muted = style(theme, Meaning::Annotation);
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let choices = self.choices();
        let label_width = choices.iter().map(|c| harness_label(c.harness).width()).max();
        let label_width = label_width.unwrap_or(0);

        let mut lines: Vec<Line<'static>> = Vec::new();
        for (n, choice) in choices.iter().enumerate() {
            let selected = n == chooser.selected;
            let label = harness_label(choice.harness);
            let pad = " ".repeat(label_width - label.width());
            let mut spans = vec![
                Span::styled(
                    if selected {
                        "> "
                    } else {
                        "  "
                    },
                    bold,
                ),
                Span::styled(format!("{} ", n + 1), muted),
                Span::styled(
                    format!("{:<3}", harness_badge(choice.harness)),
                    harness_style(theme, choice.harness),
                ),
                Span::styled(
                    format!("{label}{pad}  "),
                    if selected {
                        style(theme, Meaning::AlertError).add_modifier(Modifier::BOLD)
                    } else {
                        style(theme, Meaning::Base)
                    },
                ),
            ];
            let mut detail = choice.detail.clone();
            if let Some(why) = &choice.unavailable {
                detail = format!("{detail}: {why}");
            }
            let used: usize = spans.iter().map(Span::width).sum();
            let detail = detail
                .ellipsize(
                    Measure::Columns(budget.saturating_sub(used)),
                    Pos::End,
                    Indicator::UNICODE,
                )
                .to_string();
            spans.push(Span::styled(detail, muted));
            if choice.unavailable.is_some() {
                for span in &mut spans {
                    span.style = span.style.add_modifier(Modifier::DIM);
                }
            }
            lines.push(Line::from(spans));
        }
        let enter = match chooser.action {
            Pending::Resume => ": resume  ",
            Pending::Edit | Pending::Copy => ": edit  ",
        };
        let mut keys = vec![Span::styled("<enter>", bold), Span::styled(enter, muted)];
        if chooser.action == Pending::Resume {
            keys.extend([Span::styled("<tab>", bold), Span::styled(": edit  ", muted)]);
        }
        keys.extend([Span::styled("<esc>", bold), Span::styled(": back", muted)]);
        lines.push(Line::from(keys));

        let widest = lines.iter().map(Line::width).max().unwrap_or(0);
        let width = u16::try_from(widest + 4).unwrap_or(u16::MAX).min(room);
        // Too short for every line (a low `inline_height`): the choices before the keys, scrolled
        // to keep the selected one in sight.
        let inner = usize::from(area.height.saturating_sub(2));
        if lines.len() > inner {
            lines.truncate(choices.len());
            let shown = inner.min(lines.len());
            let offset = (chooser.selected + 1).saturating_sub(shown).min(lines.len() - shown);
            lines = lines.drain(offset..offset + shown).collect();
        }
        let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX).min(area.height);
        let popup = place(area, anchor, width, height, settings.invert);

        // Blank the rows it covers across the list (the whole width, past it), so no cut-off
        // text shows beside it.
        if let Some(list) = anchor.map(|a| a.list) {
            for y in popup.top()..popup.bottom() {
                let across = if (list.top()..list.bottom()).contains(&y) {
                    list
                } else {
                    area
                };
                f.render_widget(Clear, Rect::new(across.x, y, across.width, 1));
            }
        }
        let title = Line::from(vec![Span::styled(" Resume in ", bold)]);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(muted)
            .padding(ratatui::widgets::Padding::horizontal(1))
            .title(title);
        f.render_widget(Clear, popup);
        f.render_widget(Paragraph::new(Text::from(lines)).block(block), popup);
    }
}

/// Where the chooser goes in `area`: against the selected row, on the side away from the input
/// (above it, or below it when inverted), flipping sides when it doesn't fit, its harness badges
/// under the list's; centred without a list to anchor to, or room beside the row.
fn place(area: Rect, anchor: Option<ListAnchor>, width: u16, height: u16, invert: bool) -> Rect {
    let centred = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let Some(anchor) = anchor else {
        return centred;
    };
    let list = anchor.list;
    // The badges line up with the list's: after the border, padding, marker and digit.
    let x = anchor.badge_x.saturating_sub(6).min(list.right().saturating_sub(width)).max(list.x);
    // Over the list, or past it (over the header, or the preview) when the list is too short.
    let above = anchor.row.checked_sub(height).filter(|y| *y >= area.y);
    let below = Some(anchor.row + 1).filter(|y| y + height <= area.bottom());
    let y = if invert {
        below.or(above)
    } else {
        above.or(below)
    };
    y.map_or(centred, |y| Rect {
        x,
        y,
        width,
        height,
    })
}

/// Where the selected row was last drawn, for the chooser to open against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListAnchor {
    /// The list.
    pub list: Rect,
    /// The selected row's line.
    pub row: u16,
    /// The column the rows' harness badges are in.
    pub badge_x: u16,
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::above(false, 20, 12)]
    #[case::below_when_inverted(true, 20, 21)]
    #[case::below_when_no_room_above(false, 6, 7)]
    #[case::above_when_no_room_below(true, 27, 19)]
    fn the_chooser_opens_against_the_selected_row(
        #[case] invert: bool,
        #[case] row: u16,
        #[case] y: u16,
    ) {
        let area = Rect::new(0, 0, 100, 30);
        let list = Rect::new(2, 3, 96, 26);
        let anchor = ListAnchor {
            list,
            row,
            badge_x: 24,
        };
        let popup = place(area, Some(anchor), 40, 8, invert);
        assert_eq!((popup.x, popup.y, popup.width, popup.height), (18, y, 40, 8));
    }

    #[rstest]
    fn without_a_list_the_chooser_is_centred_and_never_overflows() {
        let area = Rect::new(0, 0, 100, 30);
        assert_eq!(place(area, None, 40, 8, false), Rect::new(30, 11, 40, 8));
        let list = Rect::new(2, 3, 96, 26);
        let anchor = ListAnchor {
            list,
            row: 12,
            badge_x: 93,
        };
        assert_eq!(place(area, Some(anchor), 40, 8, false).right(), list.right());
        // A list too short for it: it covers what's above the row.
        let short = ListAnchor {
            list: Rect::new(2, 3, 96, 5),
            row: 7,
            badge_x: 20,
        };
        assert_eq!(place(area, Some(short), 40, 6, false), Rect::new(14, 1, 40, 6));
        // No room on either side of the row: centred.
        let tiny = Rect::new(0, 0, 100, 8);
        let short = ListAnchor {
            list: Rect::new(2, 2, 96, 3),
            row: 4,
            badge_x: 20,
        };
        assert_eq!(place(tiny, Some(short), 40, 6, false), Rect::new(30, 1, 40, 6));
    }
}
