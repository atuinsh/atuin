//! `atuin ai search`: fuzzy-find an ingested agent session and reopen it, in the agent that
//! recorded it or handed off to another one.

use std::collections::HashMap;
use std::process::Command;

use atuin_ai_session::store::Store;
use atuin_ai_session::{Agent, Call, Role, Session, StopReason, Turn, resume, turns};
use atuin_client::settings::Settings;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use eyre::Result;
use futures::StreamExt;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use time::OffsetDateTime;

/// How much of a session the preview loads: its last messages, each body cut short. Bounded so
/// a session with thousands of large tool results previews as fast as a short one.
const PREVIEW_MESSAGES: u32 = 80;
const PREVIEW_CHARS: u32 = 1_500;
/// Most lines one message or tool result may take in the preview.
const PREVIEW_TEXT_LINES: usize = 6;

/// Where Enter goes: the session list, or the agent picker over the chosen session.
enum Mode {
    Search,
    /// Index into `Agent::ALL`.
    Pick(usize),
}

struct App {
    sessions: Vec<Session>,
    haystack: Vec<String>,
    query: String,
    /// Indexes into `sessions`, best match first.
    matches: Vec<usize>,
    selected: usize,
    mode: Mode,
    preview: HashMap<usize, Vec<Turn>>,
}

impl App {
    fn refilter(&mut self) {
        if self.query.trim().is_empty() {
            self.matches = (0..self.sessions.len()).collect();
        } else {
            let config = frizbee::Config::default().casing(frizbee::CaseMatching::Smart);
            let mut matcher = frizbee::Matcher::from_query(&self.query, &config);
            let hay: Vec<&str> = self.haystack.iter().map(String::as_str).collect();
            let mut found = matcher.match_list(&hay);
            found.sort_by_key(|m| (std::cmp::Reverse(m.score), m.index));
            self.matches = found.iter().map(|m| m.index as usize).collect();
        }
        self.selected = 0;
    }

    fn current(&self) -> Option<&Session> {
        self.matches.get(self.selected).map(|&i| &self.sessions[i])
    }
}

pub async fn run(query: Option<String>, settings: &Settings) -> Result<()> {
    let store = Store::open(
        &Settings::effective_data_dir().join("agent_sessions.db"),
        settings.local_timeout,
    )
    .await?;
    let sessions = store.sessions().await?;
    if sessions.is_empty() {
        println!("No agent sessions yet. Run `atuin ai ingest` first.");
        return Ok(());
    }
    let haystack = sessions
        .iter()
        .map(|s| {
            format!(
                "{} {} {} {}",
                s.agent,
                s.title.as_deref().unwrap_or_default(),
                s.cwd.as_deref().unwrap_or_default(),
                s.git_branch.as_deref().unwrap_or_default()
            )
        })
        .collect();
    let mut app = App {
        sessions,
        haystack,
        query: query.unwrap_or_default(),
        matches: Vec::new(),
        selected: 0,
        mode: Mode::Search,
        preview: HashMap::new(),
    };
    app.refilter();

    let mut terminal = ratatui::init();
    let picked = event_loop(&mut terminal, &mut app, &store).await;
    ratatui::restore();

    let Some((index, target)) = picked? else {
        return Ok(());
    };
    let session = &app.sessions[index];
    // With its ancestors: a continuation of a handoff holds only the turns added since.
    let messages = store.session_history(session.agent, &session.session_id).await?;
    let cmd = resume::reopen(target, session, &messages).await?;
    exec(cmd)
}

/// Replace this process with the agent, so the agent owns the terminal.
fn exec(mut cmd: Command) -> Result<()> {
    eprintln!(
        "→ {} {}",
        cmd.get_program().to_string_lossy(),
        cmd.get_args().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" ")
    );
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(cmd.exec().into())
    }
    #[cfg(not(unix))]
    {
        cmd.status()?;
        Ok(())
    }
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    store: &Store,
) -> Result<Option<(usize, Agent)>> {
    let mut events = EventStream::new();
    loop {
        if let Some(&i) = app.matches.get(app.selected)
            && !app.preview.contains_key(&i)
        {
            let s = &app.sessions[i];
            let tail =
                store.session_tail(s.agent, &s.session_id, PREVIEW_MESSAGES, PREVIEW_CHARS).await?;
            app.preview.insert(i, turns(&tail));
        }
        terminal.draw(|f| draw(f, app))?;

        let Some(event) = events.next().await else {
            return Ok(None);
        };
        let Event::Key(key) = event? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if let Mode::Pick(choice) = app.mode {
            match key.code {
                KeyCode::Esc => app.mode = Mode::Search,
                KeyCode::Char('c' | 'd') if ctrl => return Ok(None),
                KeyCode::Enter => {
                    if let Some(&i) = app.matches.get(app.selected) {
                        return Ok(Some((i, Agent::ALL[choice])));
                    }
                }
                KeyCode::Up | KeyCode::Left => app.mode = Mode::Pick(choice.saturating_sub(1)),
                KeyCode::Char('p' | 'k') if ctrl => app.mode = Mode::Pick(choice.saturating_sub(1)),
                KeyCode::Down | KeyCode::Right | KeyCode::Tab => {
                    app.mode = Mode::Pick((choice + 1).min(Agent::ALL.len() - 1));
                }
                KeyCode::Char('n' | 'j') if ctrl => {
                    app.mode = Mode::Pick((choice + 1).min(Agent::ALL.len() - 1));
                }
                _ => {}
            }
            continue;
        }
        match key.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::Char('c' | 'd') if ctrl => return Ok(None),
            KeyCode::Enter => {
                // Default to the agent that recorded the session.
                if let Some(s) = app.current() {
                    let original = Agent::ALL.iter().position(|&a| a == s.agent).unwrap_or(0);
                    app.mode = Mode::Pick(original);
                }
            }
            KeyCode::Up => app.selected = app.selected.saturating_sub(1),
            KeyCode::Char('p' | 'k') if ctrl => app.selected = app.selected.saturating_sub(1),
            KeyCode::Down => {
                app.selected = (app.selected + 1).min(app.matches.len().saturating_sub(1));
            }
            KeyCode::Char('n' | 'j') if ctrl => {
                app.selected = (app.selected + 1).min(app.matches.len().saturating_sub(1));
            }
            KeyCode::Backspace => {
                app.query.pop();
                app.refilter();
            }
            KeyCode::Char(c) if !ctrl => {
                app.query.push(c);
                app.refilter();
            }
            _ => {}
        }
    }
}

/// The agent picker, centred over the session list.
fn draw_picker(f: &mut Frame<'_>, app: &App, choice: usize) {
    let Some(session) = app.current() else {
        return;
    };
    let area = f.area();
    let width = 44.min(area.width);
    let height = u16::try_from(Agent::ALL.len()).unwrap_or(4) + 2;
    let popup = ratatui::layout::Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    f.render_widget(ratatui::widgets::Clear, popup);
    let items: Vec<ListItem<'_>> = Agent::ALL
        .iter()
        .map(|&agent| {
            let what = match (agent == session.agent, agent) {
                (true, _) => "resume, original",
                (false, Agent::Cursor) => "hand off via cursor-agent",
                (false, _) => "reopen",
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {:<12}", agent.to_string()),
                    Style::default().fg(agent_color(agent)),
                ),
                Span::styled(what, Style::default().dim()),
            ]))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(choice));
    f.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title(" open in "))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        popup,
        &mut state,
    );
}

fn draw(f: &mut Frame<'_>, app: &App) {
    let [input, body, footer] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(1), Constraint::Length(1)])
            .areas(f.area());
    let [list_area, preview_area] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(body);

    f.render_widget(
        Paragraph::new(format!("> {}", app.query)).block(Block::bordered().title(format!(
            " {} of {} sessions ",
            app.matches.len(),
            app.sessions.len()
        ))),
        input,
    );

    let now = OffsetDateTime::now_utc();
    let items: Vec<ListItem<'_>> = app
        .matches
        .iter()
        .map(|&i| {
            let s = &app.sessions[i];
            let dir = s
                .cwd
                .as_deref()
                .and_then(|c| std::path::Path::new(c).file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:<11}", s.agent.to_string()),
                    Style::default().fg(agent_color(s.agent)),
                ),
                Span::styled(format!("{:>4} ", age(now, s.ended_at)), Style::default().dim()),
                Span::styled(format!("{dir:<18.18} "), Style::default().fg(Color::Yellow)),
                Span::styled(
                    if s.last_stop.is_some_and(StopReason::is_unfinished) {
                        "! "
                    } else {
                        ""
                    },
                    Style::default().fg(Color::Red).bold(),
                ),
                Span::raw(s.title.clone().unwrap_or_else(|| "(untitled)".into())),
            ]))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(app.selected));
    f.render_stateful_widget(
        List::new(items)
            .block(Block::bordered())
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        list_area,
        &mut state,
    );

    draw_preview(f, app, preview_area);

    let hints: &[(&str, &str)] = match app.mode {
        Mode::Search => &[("enter", "choose agent"), ("↑↓", "move"), ("esc", "quit")],
        Mode::Pick(_) => &[("enter", "open"), ("↑↓", "choose"), ("esc", "back")],
    };
    let mut spans = Vec::new();
    for (key, what) in hints {
        spans.push(Span::styled(format!(" {key} "), Style::default().bold()));
        spans.push(Span::raw(*what));
        spans.push(Span::raw(" "));
    }
    f.render_widget(Line::from(spans), footer);

    if let Mode::Pick(choice) = app.mode {
        draw_picker(f, app, choice);
    }
}

/// Session facts on top, then the end of the conversation anchored to the bottom of the pane, so
/// the latest turns are always the ones showing however long the session is.
fn draw_preview(f: &mut Frame<'_>, app: &App, area: ratatui::layout::Rect) {
    let block = Block::bordered().title(" preview ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(s) = app.current() else {
        return;
    };
    let width = usize::from(inner.width);
    let dim = Style::default().dim();

    let mut header = vec![
        Line::from(vec![
            Span::styled(s.agent.to_string(), Style::default().fg(agent_color(s.agent)).bold()),
            Span::styled(format!("  {}", s.session_id), dim),
        ]),
        Line::from(vec![
            Span::styled(s.cwd.clone().unwrap_or_default(), Style::default().fg(Color::Yellow)),
            Span::styled(s.git_branch.as_ref().map(|b| format!("  {b}")).unwrap_or_default(), dim),
        ]),
        Line::styled(
            [
                s.model.clone().unwrap_or_default(),
                format!("{} messages", s.messages),
                format!("{} tool calls", s.tool_calls),
                if s.threads > 0 {
                    format!("{} subagents", s.threads)
                } else {
                    String::new()
                },
                span(s.ended_at - s.started_at),
            ]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" · "),
            dim,
        ),
        Line::styled("─".repeat(width), dim),
    ];
    if let Some(stop) = s.last_stop.filter(|r| r.is_unfinished()) {
        // Above the rule, so it reads as a fact about the session.
        header.insert(
            3,
            Line::styled(format!("! last turn ended: {stop}"), Style::default().fg(Color::Red)),
        );
    }
    header.truncate(usize::from(inner.height));

    let mut body: Vec<Line<'_>> = Vec::new();
    for turn in app.preview.get(&app.matches[app.selected]).into_iter().flatten() {
        // A result whose call fell outside the loaded tail arrives as a bracketed user turn.
        let orphan = turn.text.strip_prefix("[tool result]\n");
        let (who, color) = match (turn.role, orphan) {
            (_, Some(_)) => ("", Color::DarkGray),
            (Role::Assistant, _) => ("agent", Color::Magenta),
            _ => ("you", Color::Green),
        };
        let text = orphan.unwrap_or(&turn.text);
        for (n, line) in
            wrap(text, width.saturating_sub(7), PREVIEW_TEXT_LINES).into_iter().enumerate()
        {
            let label = if n == 0 {
                who
            } else {
                ""
            };
            let style = if orphan.is_some() {
                dim
            } else {
                Style::default()
            };
            body.push(Line::from(vec![
                Span::styled(format!("{label:<7}"), Style::default().fg(color).bold()),
                Span::styled(line, style),
            ]));
        }
        for call in &turn.calls {
            body.extend(call_lines(call, width));
        }
    }
    // Keep the tail: whatever does not fit is the older end.
    let room = usize::from(inner.height).saturating_sub(header.len());
    let body = body.split_off(body.len().saturating_sub(room));

    header.extend(body);
    f.render_widget(Paragraph::new(header), inner);
}

/// A tool call as one line, and the first line of what came back under it.
fn call_lines(call: &Call, width: usize) -> Vec<Line<'static>> {
    let dim = Style::default().dim();
    let what = call.shell_command().unwrap_or_else(|| match &call.input {
        // The argument a person would recognise the call by, else the whole input.
        serde_json::Value::Object(o) => {
            ["file_path", "path", "pattern", "query", "url", "description", "prompt"]
                .iter()
                .find_map(|k| o.get(*k).and_then(serde_json::Value::as_str).map(str::to_owned))
                .unwrap_or_else(|| call.input.to_string())
        }
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    });
    let name = format!("  ▸ {} ", call.name);
    let what = wrap(&what, width.saturating_sub(name.chars().count()), 1).pop().unwrap_or_default();
    let mut lines = vec![Line::from(vec![
        Span::styled(name, Style::default().fg(Color::Cyan)),
        Span::raw(what),
    ])];

    let (glyph, style) = if call.is_error {
        ("✗", Style::default().fg(Color::Red))
    } else {
        ("↳", dim)
    };
    let mut output = call.output.lines().filter(|l| !l.trim().is_empty());
    match output.next() {
        Some(first) => {
            let more = output.count();
            let suffix = if more > 0 {
                format!("  (+{more} lines)")
            } else {
                String::new()
            };
            let first = wrap(first, width.saturating_sub(6 + suffix.chars().count()), 1)
                .pop()
                .unwrap_or_default();
            lines.push(Line::from(vec![
                Span::styled(format!("    {glyph} {first}"), style),
                Span::styled(suffix, dim),
            ]));
        }
        None if call.is_error => lines.push(Line::styled(format!("    {glyph} failed"), style)),
        None => {}
    }
    lines
}

/// Word-wrap `text` to `width` columns, at most `max_lines`, ending in an ellipsis when cut.
/// Whitespace runs collapse, so a message reads as a paragraph whatever its source layout.
fn wrap(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(8);
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut cut = false;
    'words: for word in text.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            let used = line.chars().count();
            let gap = usize::from(used > 0);
            if used + gap + word.len() <= width {
                if gap == 1 {
                    line.push(' ');
                }
                line.extend(word);
                break;
            }
            if used == 0 {
                // A word longer than the pane: break it.
                line.extend(word.drain(..width));
            }
            lines.push(std::mem::take(&mut line));
            if lines.len() == max_lines {
                cut = true;
                break 'words;
            }
        }
    }
    if !line.is_empty() && lines.len() < max_lines {
        lines.push(line);
    } else if !line.is_empty() {
        cut = true;
    }
    if cut && let Some(last) = lines.last_mut() {
        while last.chars().count() >= width {
            last.pop();
        }
        last.push('…');
    }
    lines
}

fn agent_color(agent: Agent) -> Color {
    match agent {
        Agent::ClaudeCode => Color::Magenta,
        Agent::Codex => Color::Green,
        Agent::OpenCode => Color::Cyan,
        Agent::Cursor => Color::Blue,
        Agent::Pi => Color::LightRed,
    }
}

/// How long ago, compact: `3m`, `5h`, `2d`, `3w`.
fn age(now: OffsetDateTime, then: OffsetDateTime) -> String {
    let mins = (now - then).whole_minutes().max(0);
    match mins {
        m if m < 60 => format!("{m}m"),
        m if m < 60 * 24 => format!("{}h", m / 60),
        m if m < 60 * 24 * 14 => format!("{}d", m / (60 * 24)),
        m => format!("{}w", m / (60 * 24 * 7)),
    }
}

fn span(d: time::Duration) -> String {
    let mins = d.whole_minutes();
    if mins < 60 {
        format!("{mins} min")
    } else {
        format!("{}h {}m", mins / 60, mins % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_on_words_caps_lines_and_marks_the_cut() {
        assert_eq!(wrap("one two  three\nfour", 9, 6), ["one two", "three", "four"]);
        assert_eq!(wrap("one two three four five", 9, 2), ["one two", "three…"]);
        assert_eq!(wrap("abcdefghijklmnop", 8, 6), ["abcdefgh", "ijklmnop"]);
        assert_eq!(wrap("exactly8 next", 8, 1), ["exactly…"]);
        assert!(wrap("   ", 10, 6).is_empty());
    }

    #[test]
    fn calls_render_as_a_name_line_and_a_result_line() {
        let call = Call {
            id: "c".into(),
            name: "exec_command".into(),
            input: serde_json::json!({"cmd": "cargo test"}),
            output: "\nrunning 3 tests\nok\nok\n".into(),
            is_error: false,
        };
        let text: Vec<String> = call_lines(&call, 60)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(text, ["  ▸ exec_command cargo test", "    ↳ running 3 tests  (+2 lines)"]);

        let read = Call {
            id: "r".into(),
            name: "Read".into(),
            input: serde_json::json!({"file_path": "/a/b.rs", "limit": 5}),
            output: String::new(),
            is_error: false,
        };
        assert_eq!(call_lines(&read, 60).len(), 1);
        assert_eq!(call_lines(&read, 60)[0].spans[1].content, "/a/b.rs");

        let failed = Call {
            is_error: true,
            ..read
        };
        let text: String =
            call_lines(&failed, 60)[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "    ✗ failed");
    }
}
