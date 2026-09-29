//! The "Resume in" chooser: once a session is chosen (enter, or tab to edit the command), where
//! to resume it.
//!
//! Its own harness comes first and is preselected, so enter-enter resumes it; a session whose
//! transcript isn't on this machine is restored from sync behind the scenes, which the line only
//! hints at. Then every other harness installed here, to continue the session in: it is written
//! out as a new session there, its tool calls flattened into notes, and that is resumed. Harnesses
//! that aren't installed aren't listed: there is nothing to do with them. The session's own
//! harness is always listed, dimmed with the reason when it can't resume it here (a Copilot
//! session, a directory that's gone, a harness that isn't installed), and the first line that
//! works is preselected instead.
//!
//! A session that went on separately on several machines ([diverged](SessionRow::diverged))
//! has a line of its own harness per branch instead, newest first and preselected: `@buildbox ·
//! 5h · 40 msgs (latest)`, `this machine · yest · 24 msgs`. Picking one resumes that branch, in
//! the same session ([`super::catchup`]).
//!
//! The chooser keeps the key's meaning: opened with enter it resumes (or edits, without
//! `enter_accept`), opened with tab it edits, and tab in it always edits.
//!
//! Resuming a branch another host wrote to in the last few minutes
//! ([`LIVE_ELSEWHERE`](super::catchup::LIVE_ELSEWHERE)) first asks, in a [`Warning`] popup:
//! resuming it here will branch the session.

use atuin_client::ai_session::{HarnessKind, HarnessSession, Head};
use atuin_client::settings::Settings;
use atuin_client::theme::{Meaning, Theme};
use atuin_client::tui::{KeyCodeValue, SingleKey};
use atuin_common::harnesstools::continuation::Flattened;
use atuin_common::string::ellipsis::{Indicator, Pos};
use atuin_common::string::{EllipsizeExt as _, Measure};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::catchup::{self, describe, host_label};
use super::render::{harness_style, is_live, style};
use super::resumer::NotResumable;
use super::source::{SessionRow, harness_badge, harness_label};
use super::state::{InputAction, Pending, State};

/// The chooser, while it's open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chooser {
    /// The session it opened on.
    pub session: HarnessSession,
    /// The other harnesses installed here, to continue the session in: lines 2 and on (line 1
    /// resumes it in its own).
    pub targets: Vec<HarnessKind>,
    /// The selected line, from 0.
    pub selected: usize,
    /// What picking a line with enter (or its digit) does: what the key that opened it asked for.
    pub action: Pending,
    /// Whether the selection was moved by hand: if not, it moves off the first line once that
    /// turns out not to work.
    pub moved: bool,
    /// The session's branches, newest first, when it went on separately on several hosts: the
    /// first lines resume one each. Empty for a session that went one way, which has one.
    pub branches: Vec<Head>,
}

impl Chooser {
    pub fn len(&self) -> usize {
        self.targets.len() + self.own()
    }

    /// How many lines resume the session in its own harness: one per branch.
    pub fn own(&self) -> usize {
        self.branches.len().max(1)
    }

    /// The harness line `n` continues the session in; `None` for its own.
    pub fn target(&self, n: usize) -> Option<HarnessKind> {
        n.checked_sub(self.own()).and_then(|i| self.targets.get(i).copied())
    }

    /// The branch line `n` resumes, for a session with several.
    pub fn branch(&self, n: usize) -> Option<&Head> {
        self.branches.get(n)
    }
}

/// The popup warning that another host may still be working on the branch about to be resumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub session: HarnessSession,
    /// What continuing does.
    pub action: Pending,
    /// The host, as [`host_label`] names it: `@MacBook-Pro-3`.
    pub host: String,
    /// How long ago it last wrote.
    pub ago: time::Duration,
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

/// What continuing flattens, briefly: `42 tool calls become notes, reasoning dropped`.
pub fn flattened_detail(flattened: &Flattened) -> String {
    let mut parts = Vec::new();
    match flattened.tool_calls {
        0 if flattened.tool_results > 0 => parts.push("tool output dropped".to_owned()),
        0 => {}
        1 => parts.push("1 tool call becomes a note".to_owned()),
        n => parts.push(format!("{n} tool calls become notes")),
    }
    if flattened.reasoning > 0 {
        parts.push("reasoning dropped".to_owned());
    }
    parts.join(", ")
}

/// What resuming a session that is still running (see [`is_live`]) does, briefly. Claude Code
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
    /// Open the chooser on the selected session, offering its own harness and `targets` (the
    /// other harnesses installed here); picking a line with enter does `action`. The first line
    /// that works is selected.
    pub fn open_chooser(&mut self, targets: Vec<HarnessKind>, action: Pending) {
        let Some(row) = self.selected() else {
            return;
        };
        let branches = row.branches().to_vec();
        // The branch picked last time, if any.
        let selected = self
            .picked
            .get(&row.handle)
            .and_then(|p| branches.iter().position(|h| &h.source_id == p))
            .unwrap_or(0);
        self.chooser = Some(Chooser {
            session: row.handle.clone(),
            targets,
            selected,
            action,
            moved: false,
            branches,
        });
        self.settle_chooser();
    }

    /// A head's host as the chooser and the status line name it: `this machine`, `@buildbox`.
    pub fn head_host(&self, head: &Head) -> String {
        let name = |id: &str| {
            self.host_names.get(id).cloned().or_else(|| {
                self.results.iter().find(|r| r.host_id == id).map(|r| r.hostname.clone())
            })
        };
        host_label(head, &self.context.host_id, &name)
    }

    /// A branch in a line: `@buildbox · 5h · 40 msgs`.
    pub fn describe_head(&self, head: &Head) -> String {
        describe(head, (self.now)(), &|h| self.head_host(h))
    }

    /// The warning to show before resuming `row` on the branch picked (the newest, unless
    /// another was), when another host wrote to it in the last few minutes.
    pub fn live_elsewhere(&self, row: &SessionRow, action: Pending) -> Option<Warning> {
        let head = catchup::chosen_head(row, self.picked.get(&row.handle).map(AsRef::as_ref))?;
        let ago = catchup::live_elsewhere(head, &self.context.host_id, (self.now)())?;
        Some(Warning {
            session: row.handle.clone(),
            action,
            host: self.head_host(head),
            ago,
        })
    }

    /// A key while the warning is open: enter (or `y`) goes on and resumes, esc (or `n`, `q`,
    /// ctrl-c, ctrl-g) goes back to the list.
    pub fn warning_key(&mut self, key: &SingleKey) -> InputAction {
        let Some(warning) = self.warning.as_ref() else {
            return InputAction::Continue;
        };
        match (&key.code, key.ctrl) {
            (KeyCodeValue::Enter, _)
            | (KeyCodeValue::Char('m'), true)
            | (KeyCodeValue::Char('y'), false) => {
                let action = warning.action;
                self.confirmed.insert(warning.session.clone());
                self.warning = None;
                self.accept = action == Pending::Resume;
                InputAction::Pick(None, action)
            }
            (KeyCodeValue::Esc, _)
            | (KeyCodeValue::Char('c' | 'g' | '['), true)
            | (KeyCodeValue::Char('q' | 'n'), false) => {
                self.warning = None;
                self.accept = false;
                self.status = None;
                InputAction::Continue
            }
            _ => InputAction::Continue,
        }
    }

    /// Whether the session's own harness can't resume it here, and why (once its plan is known).
    pub fn original_unavailable(&self, session: &HarnessSession) -> Option<&NotResumable> {
        self.plans.get(session)?.as_ref().err()
    }

    /// Move an untouched selection off the first line once its plan says it can't be picked.
    pub fn settle_chooser(&mut self) {
        let Some(chooser) = &self.chooser else {
            return;
        };
        if chooser.moved
            || chooser.target(chooser.selected).is_some()
            || chooser.targets.is_empty()
            || self.original_unavailable(&chooser.session).is_none()
        {
            return;
        }
        if let Some(chooser) = self.chooser.as_mut() {
            chooser.selected = chooser.own();
        }
    }

    /// The chooser's lines.
    pub fn choices(&self) -> Vec<Choice> {
        let Some(chooser) = &self.chooser else {
            return Vec::new();
        };
        let session = &chooser.session;
        let live = self
            .selected()
            .filter(|r| r.handle == *session)
            .is_some_and(|r| is_live((self.now)(), r));
        let unavailable = match self.plans.get(session) {
            Some(Err(why)) => Some(short_reason(why)),
            _ => None,
        };
        let branches = chooser.branches.iter().enumerate().map(|(n, head)| Choice {
            harness: session.harness,
            detail: if n == 0 {
                format!("{} (latest)", self.describe_head(head))
            } else {
                self.describe_head(head)
            },
            unavailable: unavailable.clone(),
        });
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
            Some(Ok(flattened)) => flattened_detail(flattened),
            _ => String::new(),
        };
        let continued = chooser.targets.iter().map(|target| Choice {
            harness: *target,
            detail: if flattened.is_empty() {
                "continue".to_owned()
            } else {
                format!("continue, {flattened}")
            },
            unavailable: None,
        });
        if chooser.branches.is_empty() {
            std::iter::once(original).chain(continued).collect()
        } else {
            branches.chain(continued).collect()
        }
    }

    /// A key while the chooser is open: move (up/down, ctrl-p/ctrl-n, k/j), pick (enter does
    /// what opened the chooser, tab edits, ctrl-y copies the command, a digit picks its line),
    /// or go back to the list (esc, q, ctrl-c, ctrl-g).
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
                chooser.selected = chooser.selected.saturating_sub(1);
                chooser.moved = true;
                return InputAction::Continue;
            }
            (KeyCodeValue::Down, _)
            | (KeyCodeValue::Char('n'), true)
            | (KeyCodeValue::Char('j'), false) => {
                chooser.selected = (chooser.selected + 1).min(last);
                chooser.moved = true;
                return InputAction::Continue;
            }
            (KeyCodeValue::Char(c @ '1'..='9'), false) => {
                let n = c.to_digit(10).and_then(|n| usize::try_from(n).ok()).unwrap_or(0);
                if n > chooser.len() {
                    return InputAction::Continue;
                }
                chooser.selected = n - 1;
                chooser.moved = true;
                chooser.action
            }
            (KeyCodeValue::Enter, _) | (KeyCodeValue::Char('m'), true) => chooser.action,
            (KeyCodeValue::Tab, _) => Pending::Edit,
            (KeyCodeValue::Char('y'), true) => Pending::Copy,
            _ => return InputAction::Continue,
        };
        let target = chooser.target(chooser.selected);
        let branch = chooser.branch(chooser.selected).map(|h| h.source_id.clone());
        let session = chooser.session.clone();
        if target.is_none()
            && pick != Pending::Copy
            && self.original_unavailable(&session).is_some()
        {
            // Its line already says why.
            let label = harness_label(session.harness);
            self.status = Some((
                format!("{label} can't resume it here: pick another line"),
                Meaning::AlertError,
            ));
            return InputAction::Continue;
        }
        self.accept = pick == Pending::Resume;
        if pick != Pending::Copy {
            self.chooser = None;
        }
        if target.is_none() {
            match branch {
                Some(head) => {
                    self.picked.insert(session, head);
                }
                None => {
                    self.picked.remove(&session);
                }
            }
        }
        InputAction::Pick(target, pick)
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
        if !chooser.branches.is_empty() {
            lines.push(Line::from(Span::styled(
                format!(
                    "{} branches: it went on separately on several machines",
                    chooser.branches.len()
                ),
                muted,
            )));
        }
        lines.extend(choices.iter().enumerate().map(|(n, choice)| {
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
            Line::from(spans)
        }));
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

    /// The warning that another host may still be working on the session, centred.
    pub fn draw_warning(&self, f: &mut Frame, theme: &Theme) {
        let Some(warning) = &self.warning else {
            return;
        };
        let area = f.area();
        let muted = style(theme, Meaning::Annotation);
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let ago = match warning.ago.whole_minutes() {
            0 => "just now".to_owned(),
            n => format!("{n}m ago"),
        };
        let lines = vec![
            Line::from(vec![
                Span::styled("Still active on ", style(theme, Meaning::Base)),
                Span::styled(warning.host.clone(), bold),
                Span::styled(format!(" ({ago})."), style(theme, Meaning::Base)),
            ]),
            Line::from(Span::styled(
                "Resuming here will branch the session.",
                style(theme, Meaning::Base),
            )),
            Line::default(),
            Line::from(vec![
                Span::styled("<enter>", bold),
                Span::styled(": continue  ", muted),
                Span::styled("<esc>", bold),
                Span::styled(": back", muted),
            ]),
        ];
        let widest = lines.iter().map(Line::width).max().unwrap_or(0);
        let width = u16::try_from(widest + 4).unwrap_or(u16::MAX).min(area.width);
        let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX).min(area.height);
        let popup = place(area, None, width, height, false);
        let title = Line::from(vec![Span::styled(
            " Still active elsewhere ",
            style(theme, Meaning::AlertWarn).add_modifier(Modifier::BOLD),
        )]);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(style(theme, Meaning::AlertWarn))
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
    #[case(Flattened { tool_calls: 42, tool_results: 42, reasoning: 3 }, "42 tool calls become notes, reasoning dropped")]
    #[case(Flattened { tool_calls: 1, tool_results: 1, reasoning: 0 }, "1 tool call becomes a note")]
    #[case(Flattened { tool_calls: 0, tool_results: 2, reasoning: 0 }, "tool output dropped")]
    #[case(Flattened::default(), "")]
    fn flattened_details_are_short(#[case] flattened: Flattened, #[case] expected: &str) {
        assert_eq!(flattened_detail(&flattened), expected);
    }

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
