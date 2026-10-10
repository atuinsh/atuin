//! The reader: a session's whole conversation in the detail pane and Inspect, scrolling, opened at
//! the search's match.
//!
//! It renders a transcript's entries as far as the view needs and keeps them between frames
//! ([`Rendered`], one per pane), so scrolling a long session renders each entry once. A live
//! session read again keeps what was rendered of it, when that hasn't changed.

use std::sync::Arc;

use atuin_client::ai_session::HarnessKind;
use atuin_client::theme::{Meaning, Theme};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::render::{Body, harness_style, style};
use super::source::{SessionRow, Transcript, TranscriptEntry, harness_label};
use super::state::{Pane, State};
use super::{markdown, title};

/// What the reader rendered, and for what: rendered again from the start when any of it changes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    session: atuin_client::ai_session::HarnessSession,
    width: usize,
    indent: usize,
    /// The query text of the results on screen, whose words are highlighted.
    query: String,
}

/// A pane's reader's lines rendered so far: a run of the transcript's entries, from the one it
/// opened at, grown downwards as it scrolls down and upwards as it scrolls up, so opening deep in
/// a long session renders only what's around the match.
#[derive(Debug)]
pub struct Rendered {
    key: Key,
    /// The transcript they were rendered from.
    transcript: Arc<Transcript>,
    /// The first entry rendered.
    first: usize,
    lines: Vec<Line<'static>>,
    /// The line each entry rendered (from `first`) starts on: its heading, when it has one.
    starts: Vec<usize>,
}

/// Who an entry is from: a heading goes over each change of speaker.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Speaker {
    User,
    Agent,
    Summary,
}

fn speaker(entry: &TranscriptEntry) -> Speaker {
    match entry {
        TranscriptEntry::Prompt(_) => Speaker::User,
        TranscriptEntry::Reply(_) | TranscriptEntry::Tools(_) => Speaker::Agent,
        TranscriptEntry::Summary(_) => Speaker::Summary,
    }
}

/// How much the text sits in from its heading.
const TEXT_INDENT: usize = 2;

/// The most lines one entry renders to: past that it is cut with `…`, so one enormous paste
/// can't hold up the frame.
const ENTRY_MAX_LINES: usize = 2000;

/// A run of tool calls on one line: `Read ×3 · Edit · Bash ×2`.
fn tools_line(tools: &[(String, usize)]) -> String {
    tools
        .iter()
        .map(|(name, n)| {
            if *n > 1 {
                format!("{name} ×{n}")
            } else {
                name.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

impl Rendered {
    fn new(key: Key, transcript: Arc<Transcript>, first: usize) -> Self {
        Self {
            key,
            transcript,
            first,
            lines: Vec::new(),
            starts: Vec::new(),
        }
    }

    /// The entry after the last rendered.
    fn end(&self) -> usize {
        self.first + self.starts.len()
    }

    /// Whether the entries rendered are the same in `transcript`: a live session read again
    /// mostly adds to its end, so what was rendered of it stands.
    fn still_holds(&self, transcript: &Transcript) -> bool {
        let rendered = self.first..self.end();
        transcript.entries.get(rendered.clone()) == self.transcript.entries.get(rendered)
    }

    /// Render the next entry, after those rendered.
    fn push(&mut self, harness: HarnessKind, theme: &Theme) {
        let i = self.end();
        if let Some((lines, start)) = self.entry_lines(i, harness, theme) {
            self.starts.push(self.lines.len() + start);
            self.lines.extend(lines);
        }
    }

    /// Render the entry before those rendered. How many lines that added above them.
    fn prepend(&mut self, harness: HarnessKind, theme: &Theme) -> usize {
        let Some(i) = self.first.checked_sub(1) else {
            return 0;
        };
        let Some((lines, start)) = self.entry_lines(i, harness, theme) else {
            return 0;
        };
        let added = lines.len();
        for s in &mut self.starts {
            *s += added;
        }
        self.starts.insert(0, start);
        self.lines.splice(0..0, lines);
        self.first = i;
        added
    }

    /// Entry `i`'s lines (a blank line over it, when it has one, then its heading, when the
    /// speaker changes, then its text), and the line it starts on among them.
    fn entry_lines(
        &self,
        i: usize,
        harness: HarnessKind,
        theme: &Theme,
    ) -> Option<(Vec<Line<'static>>, usize)> {
        let entries = &self.transcript.entries;
        let entry = entries.get(i)?;
        let mut lines = Vec::new();
        let Key {
            width,
            indent,
            query,
            ..
        } = &self.key;
        let pad = " ".repeat(*indent);
        let text_pad = " ".repeat(indent + TEXT_INDENT);
        let text_width = width.saturating_sub(indent + TEXT_INDENT).max(1);
        let base = style(theme, Meaning::Base);
        let muted = style(theme, Meaning::Annotation);

        // A blank line between entries (none over the first), but a run of tool calls sits
        // right under the text before it.
        let who = speaker(entry);
        let follows = i > 0 && speaker(&entries[i - 1]) == who;
        if i > 0 && !(follows && matches!(entry, TranscriptEntry::Tools(_))) {
            lines.push(Line::default());
        }
        let start = lines.len();
        if !follows {
            let heading = match who {
                Speaker::User => Span::styled("You", base.add_modifier(Modifier::BOLD)),
                Speaker::Agent => Span::styled(
                    harness_label(harness),
                    harness_style(theme, harness).add_modifier(Modifier::BOLD),
                ),
                Speaker::Summary => Span::styled("Summary", muted.add_modifier(Modifier::BOLD)),
            };
            lines.push(Line::from(vec![Span::raw(pad), heading]));
        }

        let body = match entry {
            TranscriptEntry::Tools(tools) => markdown::wrap_plain(
                &[Span::styled(format!("⚙ {}", tools_line(tools)), muted)],
                text_width,
                0,
                muted,
            ),
            TranscriptEntry::Prompt(text)
            | TranscriptEntry::Reply(text)
            | TranscriptEntry::Summary(text) => {
                let highlights = if query.is_empty() {
                    Vec::new()
                } else {
                    title::highlights(text, query)
                };
                let text_style: Style = if who == Speaker::Summary {
                    muted
                } else {
                    base
                };
                let opts = markdown::Opts {
                    width: text_width,
                    max_lines: ENTRY_MAX_LINES,
                    spacing: true,
                    urls: true,
                };
                markdown::render(text, &highlights, opts, &markdown::Styles::new(theme, text_style))
            }
        };
        for line in body {
            let mut spans = vec![Span::raw(text_pad.clone())];
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        }
        Some((lines, start))
    }
}

/// An entry's text: none for tool calls.
fn text(entry: &TranscriptEntry) -> Option<&str> {
    match entry {
        TranscriptEntry::Prompt(t) | TranscriptEntry::Reply(t) | TranscriptEntry::Summary(t) => {
            Some(t.as_str())
        }
        TranscriptEntry::Tools(_) => None,
    }
}

/// `text`'s words, one space apart: how a search's snippet quotes it.
fn words(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The entry the reader opens at: the one from the message the search matched, when the query is
/// in it (a word of the session's title or its repository matches every message, so the search's
/// pick is then any); else (a match in a grouped session, or a source that doesn't say) the one
/// the match's snippet quotes, then the first with a word of the query in it; else the first.
fn opening_entry(transcript: &Transcript, row: &SessionRow, query: &str) -> usize {
    if query.is_empty() {
        return 0;
    }
    let entries = &transcript.entries;
    // The message's entries (its text, then a run of tool calls it starts, which may have begun
    // in a message before it): the one with the query in its text, else its tool calls (which
    // have no text to check: the match may be in a call's input).
    if let Some(m) = row.matched_at
        && let Some(last) = transcript.entry_at(m)
    {
        let first = transcript.messages.iter().position(|x| *x >= m).unwrap_or(last).min(last);
        let has =
            |i: &usize| text(&entries[*i]).is_some_and(|t| !title::highlights(t, query).is_empty());
        let tools = |i: &usize| text(&entries[*i]).is_none();
        if let Some(at) = (first..=last).find(has).or_else(|| (first..=last).find(tools)) {
            return at;
        }
    }
    // A snippet collapses whitespace and marks where it was cut with `…`.
    let quoted =
        row.matched.as_ref().map(|m| words(m.text.trim_matches('…'))).filter(|m| !m.is_empty());
    quoted
        .and_then(|m| entries.iter().position(|e| text(e).is_some_and(|t| words(t).contains(&m))))
        .or_else(|| {
            entries
                .iter()
                .position(|e| text(e).is_some_and(|t| !title::highlights(t, query).is_empty()))
        })
        .unwrap_or(0)
}

impl State {
    /// Whether `row`'s search match isn't in its conversation's text: the reader then shows the
    /// match's snippet over the conversation, saying why the session is listed.
    pub(super) fn match_elsewhere(&mut self, row: &SessionRow) -> bool {
        let query = &self.applied_query;
        if query.is_empty() || row.matched.is_none() {
            return false;
        }
        let Some(read) = self.transcripts.get_mut(&row.handle) else {
            return false;
        };
        if let Some((q, elsewhere)) = &read.elsewhere
            && q == query
        {
            return *elsewhere;
        }
        let elsewhere = !read
            .transcript
            .entries
            .iter()
            .any(|e| text(e).is_some_and(|t| !title::highlights(t, query).is_empty()));
        read.elsewhere = Some((query.clone(), elsewhere));
        elsewhere
    }

    /// The reader for `row` in `pane`: `height` lines of its conversation in `width` columns,
    /// indented by `indent`, from where the pane was scrolled to, or, the first time the pane
    /// shows `row` for the results on screen, from the search's match. `None` until its
    /// transcript is read (or when it has none), for the preview to stand in.
    pub(super) fn reader_body(
        &mut self,
        pane: Pane,
        row: &SessionRow,
        width: usize,
        indent: usize,
        height: usize,
        theme: &Theme,
    ) -> Option<Body> {
        let transcript = Arc::clone(&self.transcripts.get(&row.handle)?.transcript);
        if transcript.entries.is_empty() {
            return None;
        }
        let key = Key {
            session: row.handle.clone(),
            width,
            indent,
            query: self.applied_query.clone(),
        };
        let harness = row.handle.harness;
        let scroll = &mut self.scrolls[pane as usize];
        let reading = (row.handle.clone(), key.query.clone());
        let same = scroll.reading.as_ref() == Some(&reading)
            && scroll.session.as_ref() == Some(&row.handle);
        scroll.reading = Some(reading);
        let offset = scroll.offset;
        let open = (!same).then(|| opening_entry(&transcript, row, &key.query));

        let slot = &mut self.readers[pane as usize];
        let kept = slot.as_ref().is_some_and(|r| {
            r.key == key
                && r.still_holds(&transcript)
                // Opening before what was rendered starts again from there.
                && open.is_none_or(|open| open >= r.first)
        });
        // Rendered again (another width, or the session changed under it), the reader stays at
        // the entry it was showing.
        let open = open.or_else(|| {
            let old = slot.as_ref().filter(|r| !kept && r.key.session == key.session)?;
            let at = old.first + old.starts.partition_point(|s| *s <= offset).checked_sub(1)?;
            (at < transcript.entries.len()).then_some(at)
        });
        if !kept {
            *slot = Some(Rendered::new(key, Arc::clone(&transcript), open.unwrap_or(0)));
        }
        let rendered = slot.as_mut()?;
        rendered.transcript = Arc::clone(&transcript);

        let entries = transcript.entries.len();
        let mut want = match open {
            Some(open) => {
                while rendered.end() <= open {
                    rendered.push(harness, theme);
                }
                rendered.starts[open - rendered.first]
            }
            None => offset,
        };
        // Near the top of what's rendered, the entries before it, a couple of screens' worth.
        if want < height {
            let mut added = 0;
            while added < 2 * height && rendered.first > 0 {
                added += rendered.prepend(harness, theme);
            }
            want += added;
        }
        // A screen past what's shown, so the scrollbar knows there's more.
        while rendered.lines.len() < want + 2 * height && rendered.end() < entries {
            rendered.push(harness, theme);
        }

        let len = rendered.lines.len();
        let more = rendered.end() < entries;
        let offset = want.min(len.saturating_sub(height));
        Some(Body {
            lines: rendered.lines.iter().skip(offset).take(height).cloned().collect(),
            offset,
            len,
            more,
        })
    }
}

#[cfg(test)]
impl Rendered {
    /// How many lines are rendered.
    pub fn len(&self) -> usize {
        self.lines.len()
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::super::source::{Snippet, TranscriptBuilder};
    use super::*;

    #[rstest]
    fn runs_of_tool_calls_fold_into_one_line() {
        let mut t = TranscriptBuilder::default();
        t.prompt("fix it");
        t.tool("Read");
        t.tool("Read");
        t.tool("Edit");
        t.reply("done");
        t.tool("Bash");
        let transcript = t.finish();
        assert_eq!(transcript.entries.len(), 4);
        let TranscriptEntry::Tools(tools) = &transcript.entries[1] else {
            panic!("{transcript:?}");
        };
        assert_eq!(tools_line(tools), "Read ×2 · Edit");
    }

    /// A transcript of a prompt (message 0), a run of tool calls (1 and 3, results between),
    /// a long reply (5), another prompt (6), and a reply that goes on to call a tool (7).
    fn transcript() -> Transcript {
        let mut t = TranscriptBuilder::default();
        t.at(0);
        t.prompt("write a test for the clock module");
        t.at(1);
        t.tool("Read");
        t.at(3);
        t.tool("Edit");
        t.at(5);
        t.reply("The test reads the wall clock directly,\nso I switched it to a fixed clock.");
        t.at(6);
        t.prompt("thanks");
        t.at(7);
        t.reply("Kept the fixed clock for the other tests too.");
        t.tool("Edit");
        t.finish()
    }

    fn row(matched: Option<&str>, matched_at: Option<u64>) -> SessionRow {
        let mut row = super::super::fake::row(HarnessKind::ClaudeCode, "s", "t");
        row.matched = matched.map(|m| Snippet {
            text: m.to_owned(),
            highlights: Vec::new(),
        });
        row.matched_at = matched_at;
        row
    }

    #[rstest]
    // The search says which message matched, and the query is in it: there.
    #[case::the_message_matched(row(None, Some(5)), 2)]
    // A tool call folded into a run: the run (whose call's input may hold the match).
    #[case::a_call_in_a_run(row(None, Some(3)), 1)]
    // A message whose text has the query, then a tool call: the text, not the calls under it.
    #[case::text_before_its_calls(row(None, Some(7)), 4)]
    // A message the query isn't in (a title word matches them all): not trusted.
    #[case::a_message_without_the_query(row(None, Some(6)), 0)]
    // A database snippet: whitespace collapsed, cut with `…`, quoting the reply.
    #[case::the_snippet_quoted(
        row(Some("…reads the wall clock directly, so I switched…"), None),
        2
    )]
    // Neither: the first entry with a word of the query.
    #[case::a_word_of_the_query(row(None, None), 0)]
    fn the_reader_opens_at_the_match(#[case] row: SessionRow, #[case] want: usize) {
        assert_eq!(opening_entry(&transcript(), &row, "fixed clock"), want);
    }
}
