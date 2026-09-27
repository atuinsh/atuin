//! Rendering tests: the picker drawn from the fake source into ratatui's `TestBackend`.
//!
//! `ATUIN_RESUME_DUMP=<file> cargo test -p atuin-ai resume_tui::tests::dump_frames` writes the
//! frames as text for eyeballing.

use atuin_client::settings::{KeymapMode, Settings, Style as UiStyle};
use atuin_client::theme::ThemeManager;
use atuin_client::tui::Cursor;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use rstest::rstest;

use super::fake::{self, FakeResumer, FakeSource};
use super::resumer::Resumer;
use super::source::SessionSource;
use super::state::State;

fn settings() -> Settings {
    let mut s = Settings::utc();
    // The built-in default is compact; most tests look at the full style.
    s.style = UiStyle::Auto;
    s.enter_accept = true;
    s.show_preview = true;
    s.max_preview_height = 4;
    s
}

/// A picker state after its searches, previews and children have come back.
async fn loaded(settings: &Settings, query: &str, tab: usize) -> State {
    let source = FakeSource::new();
    let resumer = FakeResumer::default();
    let mut state = State::new(settings, fake::context(), "");
    state.now = Box::new(fake::now);
    state.input = Cursor::from(query.to_owned());
    state.input.end();
    while let Some((generation, mode, filter)) = state.next_search() {
        let rows = source.search(&filter).await.unwrap();
        state.apply_results(generation, mode, rows);
    }
    for row in &state.results {
        state.previews.insert(row.handle.clone(), source.preview(&row.handle).await.unwrap());
        let children = source.children(&row.handle, true).await.unwrap();
        state.children.insert(row.handle.clone(), children);
        state.plans.insert(row.handle.clone(), resumer.plan(row).await);
    }
    state.tab_index = tab;
    state
}

fn render(state: &mut State, settings: &Settings, width: u16, height: u16) -> Buffer {
    let mut themes = ThemeManager::new(None, None);
    let theme = themes.load_theme("default", None);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| state.draw(f, settings, theme)).unwrap();
    terminal.backend().buffer().clone()
}

fn text(buf: &Buffer) -> String {
    let area = buf.area;
    (0..area.height)
        .map(|y| {
            let line: String =
                (0..area.width).map(|x| buf[(x, y)].symbol().to_owned()).collect::<String>();
            line.trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn frame(settings: &Settings, query: &str, tab: usize, w: u16, h: u16) -> String {
    let mut state = loaded(settings, query, tab).await;
    text(&render(&mut state, settings, w, h))
}

#[rstest]
#[tokio::test]
async fn full_frame_has_history_search_chrome() {
    let out = frame(&settings(), "", 0, 100, 30).await;
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].contains(&format!("Atuin v{}", env!("CARGO_PKG_VERSION"))), "{out}");
    assert!(lines[0].contains("<esc>: exit, <tab>: edit, <enter>: resume, <ctrl-o>: inspect"));
    assert!(lines[0].trim_end().ends_with("sessions"), "{out}");
    assert!(lines[1].contains("Search") && lines[1].contains("Inspect"));
    assert!(lines[2].trim_start().starts_with('╭'), "list block opens the box: {out}");
    assert!(out.contains("[   WORKSPACE    ]"), "{out}");
    assert!(out.lines().last().unwrap().trim_start().starts_with('╰'), "{out}");
}

#[rstest]
#[tokio::test]
async fn rows_show_badges_children_live_and_other_hosts() {
    let out = frame(&settings(), "", 0, 100, 30).await;
    // The newest session sits at the bottom (not inverted), selected.
    let selected = out.lines().find(|l| l.contains(" > ")).unwrap();
    assert!(selected.contains("● 30s"), "live dot: {selected}");
    assert!(selected.contains("CC") && selected.contains("+4"), "{selected}");
    assert!(selected.contains("Add an interactive resume picker"), "{selected}");
    assert!(
        selected.contains("atuin") && selected.contains("ai-resume") && selected.contains("142")
    );
    // Workspace hides the dotfiles and remote sessions.
    assert!(!out.contains("dotfiles"), "{out}");
    assert!(!out.contains("@buildbox"), "{out}");
    // Subagents fold into the root instead of flooding the list.
    assert!(!out.contains("Explore: find"), "{out}");
}

/// Other hosts' sessions show their host, and aren't dimmed: they resume by being restored
/// from sync, which the selected one says.
#[rstest]
#[tokio::test]
async fn global_mode_shows_other_hosts_as_restorable() {
    let mut state = loaded(&settings(), "", 0).await;
    state.mode = atuin_client::settings::AiSessionFilterMode::Global;
    let source = FakeSource::new();
    let (generation, mode, filter) = state.next_search().unwrap();
    state.apply_results(generation, mode, source.search(&filter).await.unwrap());
    let buf = render(&mut state, &settings(), 100, 30);
    let out = text(&buf);
    let (y, line) = out.lines().enumerate().find(|(_, l)| l.contains("@buildbox")).unwrap();
    assert!(line.contains("Bisect the aarch64"), "{line}");
    let x = u16::try_from(line.find("Bisect").unwrap()).unwrap();
    let cell = &buf[(x, u16::try_from(y).unwrap())];
    assert!(!cell.modifier.contains(ratatui::style::Modifier::DIM), "remote rows aren't dimmed");
    assert!(out.contains("@laptop"));

    let remote = state.results.iter().position(|r| r.host_id != fake::THIS_HOST_ID).unwrap();
    state.list.selected = remote;
    let out = text(&render(&mut state, &settings(), 100, 30));
    assert!(out.contains("restores from sync"), "{out}");
}

/// Inspecting another host's session says it will be restored from sync, and how it resumes.
#[rstest]
#[tokio::test]
async fn inspect_says_a_remote_session_is_restored() {
    let mut s = settings();
    s.ai.sessions.filter_mode = Some(atuin_client::settings::AiSessionFilterMode::Global);
    let mut state = loaded(&s, "aarch64", 1).await;
    let out = text(&render(&mut state, &s, 100, 30));
    assert!(out.contains("Restore   from sync, when resumed"), "{out}");
    assert!(out.contains("Resume    cd -- "), "{out}");
}

#[rstest]
#[tokio::test]
async fn preview_shows_first_prompt_match_and_last_reply() {
    let out = frame(&settings(), "subagents", 0, 100, 30).await;
    assert!(out.contains("first  Build `atuin ai resume`"), "{out}");
    assert!(out.contains("match  …Group forks and subagents under their root session"), "{out}");
    assert!(out.contains("last   Rows now fold forks and subagents"), "{out}");

    // A match inside the first prompt highlights it there instead of repeating it.
    let out = frame(&settings(), "flaky", 0, 100, 30).await;
    assert!(out.contains("first  record::sync::tests::sync_down is flaky on CI"), "{out}");
    assert!(!out.contains("match  "), "{out}");
    assert!(out.contains("last   It depends on wall-clock ordering"), "{out}");
}

#[rstest]
#[tokio::test]
async fn match_highlights_are_bold() {
    let mut state = loaded(&settings(), "flaky", 0).await;
    let buf = render(&mut state, &settings(), 100, 30);
    let out = text(&buf);
    let (y, line) = out.lines().enumerate().find(|(_, l)| l.contains("first  ")).unwrap();
    let y = u16::try_from(y).unwrap();
    let at =
        |needle: &str| u16::try_from(line[..line.find(needle).unwrap()].chars().count()).unwrap();
    assert!(buf[(at("flaky"), y)].modifier.contains(ratatui::style::Modifier::BOLD));
    assert!(!buf[(at("record"), y)].modifier.contains(ratatui::style::Modifier::BOLD));
}

#[rstest]
#[tokio::test]
async fn tokens_render_as_chips() {
    let mut state = loaded(&settings(), "h:codex flaky", 0).await;
    let buf = render(&mut state, &settings(), 100, 30);
    let out = text(&buf);
    let (y, line) = out.lines().enumerate().find(|(_, l)| l.contains("h:codex")).unwrap();
    let x = u16::try_from(line[..line.find("h:codex").unwrap()].chars().count()).unwrap();
    let y = u16::try_from(y).unwrap();
    assert!(buf[(x, y)].modifier.contains(ratatui::style::Modifier::REVERSED));
    let plain = x + u16::try_from("h:codex ".len()).unwrap();
    assert!(!buf[(plain, y)].modifier.contains(ratatui::style::Modifier::REVERSED));
    assert!(out.contains("Fix the flaky sync test"));
    assert!(!out.contains("Add an interactive"));
}

#[rstest]
#[tokio::test]
async fn inspect_tab_shows_metadata_command_and_children() {
    let out = frame(&settings(), "", 1, 100, 30).await;
    assert!(out.contains("Session   7f3c9a12-5be0-4d7e-9c41-0a8e2b6f4d10  Claude Code"), "{out}");
    assert!(out.contains("Host      wintermute  (this host)"));
    assert!(out.contains(
        "Resume    cd -- /home/ellie/src/atuin && claude --resume \
         7f3c9a12-5be0-4d7e-9c41-0a8e2b6f4d10"
    ));
    assert!(out.contains("Children (4)"));
    assert!(out.contains("├─ subagent  Review the resume picker diff"), "{out}");
    assert!(out.contains("└─ fork      Add an interactive resume picker to atuin ai (fork)"));
    // The fork's own subagent nests under it.
    assert!(out.contains("   └─ subagent  Explore: how ratatui's Table highlights cells"));
    assert!(out.contains("Tokens    in 327k · out 58k · cache 2.9M"), "{out}");
    assert!(out.contains("Activity  "), "{out}");
    assert!(out.contains("<esc>: back"));
}

#[rstest]
#[tokio::test]
async fn inspect_explains_unresumable_sessions() {
    let mut s = settings();
    s.ai.sessions.filter_mode = Some(atuin_client::settings::AiSessionFilterMode::Global);
    let mut state = loaded(&s, "theme preview", 1).await;
    let out = text(&render(&mut state, &s, 100, 30));
    assert!(out.contains("not resumable: the session's directory is gone"), "{out}");
}

#[rstest]
#[tokio::test]
async fn compact_and_inline_heights() {
    let mut s = settings();
    // Auto style goes compact under 14 rows, like history search.
    let out = frame(&s, "", 0, 80, 13).await;
    assert!(!out.contains('╭'), "compact has no borders: {out}");
    assert!(out.contains("[   WORKSPACE    ]"));

    // 80x14 is still full.
    let out = frame(&s, "", 0, 80, 14).await;
    assert!(out.contains('╭'));

    s.style = UiStyle::Compact;
    let out = frame(&s, "", 0, 80, 14).await;
    assert!(!out.contains('╭'));
}

#[rstest]
#[tokio::test]
async fn invert_puts_the_input_on_top() {
    let mut s = settings();
    s.invert = true;
    let out = frame(&s, "", 0, 100, 30).await;
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[1].contains("WORKSPACE"), "input first when inverted: {out}");
    // The best match is at the top.
    assert!(lines[3].contains(" > ") && lines[3].contains("Add an interactive"), "{out}");
    assert!(lines.last().unwrap().contains("<esc>: exit"));
}

#[rstest]
#[tokio::test]
async fn vim_normal_highlights_the_whole_row() {
    let mut s = settings();
    s.keymap_mode = KeymapMode::VimNormal;
    let mut state = loaded(&s, "", 0).await;
    let buf = render(&mut state, &s, 100, 30);
    let out = text(&buf);
    let y = out.lines().position(|l| l.contains(" > ")).unwrap();
    let cell = &buf[(10, u16::try_from(y).unwrap())];
    assert!(cell.modifier.contains(ratatui::style::Modifier::REVERSED));
}

#[rstest]
#[tokio::test]
async fn no_sessions_in_workspace_widens() {
    let mut ctx = fake::context();
    ctx.cwd = "/home/ellie/src/empty".into();
    ctx.git_root = Some("/home/ellie/src/empty".into());
    let source = FakeSource::new();
    let s = settings();
    let mut state = State::new(&s, ctx, "");
    state.now = Box::new(fake::now);
    while let Some((generation, mode, filter)) = state.next_search() {
        state.apply_results(generation, mode, source.search(&filter).await.unwrap());
    }
    let out = text(&render(&mut state, &s, 100, 30));
    assert!(out.contains("[   WS→GLOBAL    ]"), "{out}");
    assert!(out.contains("@buildbox"));
}

#[rstest]
#[tokio::test]
async fn wide_terminals_split_the_list_and_a_detail_pane() {
    let out = frame(&settings(), "subagents", 0, 140, 30).await;
    let lines: Vec<&str> = out.lines().collect();
    // The divider joins the box's borders.
    assert!(lines[2].contains('┬'), "{out}");
    let selected = lines.iter().find(|l| l.contains(" > ")).unwrap();
    assert!(selected.contains('│'), "list and pane side by side: {selected}");
    assert!(out.contains("Claude Code · claude-opus-4-5"), "{out}");
    assert!(out.contains("First prompt"));
    assert!(out.contains("Match"));
    assert!(out.contains("Last reply"));
    // No preview strip under the input.
    assert!(!out.contains("first  "));

    // Narrower terminals keep the strip.
    let out = frame(&settings(), "", 0, 119, 30).await;
    assert!(out.contains("first  "));
}

#[rstest]
#[tokio::test]
async fn overflowing_lists_get_a_scrollbar() {
    let mut s = settings();
    s.ai.sessions.filter_mode = Some(atuin_client::settings::AiSessionFilterMode::Global);
    let mut state = loaded(&s, "", 0).await;
    let buf = render(&mut state, &s, 100, 14);
    let right: String = (0..14).map(|y| buf[(98, y)].symbol().to_owned()).collect();
    assert!(right.contains('┃'), "thumb on the right border: {right:?}");
}

#[rstest]
#[tokio::test]
async fn dump_frames() {
    let s = settings();
    let mut frames = Vec::new();
    let mut compact = s.clone();
    compact.style = UiStyle::Compact;
    for (label, settings, query, tab, w, h) in [
        ("full, 100x30", &s, "", 0, 100, 30),
        ("full, query 'flaky wall', 100x30", &s, "flaky wall", 0, 100, 30),
        ("full, inspect (ctrl-o), 100x30", &s, "", 1, 100, 30),
        ("full, 80x14 (inline_height = 14)", &s, "", 0, 80, 14),
        ("full, wide split, query 'subagents', 140x32", &s, "subagents", 0, 140, 32),
        ("compact, 100x30", &compact, "", 0, 100, 30),
        ("compact, 80x14 (inline_height = 14)", &compact, "", 0, 80, 14),
    ] {
        frames.push(format!("=== {label} ===\n{}", frame(settings, query, tab, w, h).await));
    }
    let mut global = s.clone();
    global.ai.sessions.filter_mode = Some(atuin_client::settings::AiSessionFilterMode::Global);
    frames.push(format!(
        "=== global 'h:claude', 100x30 ===\n{}",
        frame(&global, "h:claude", 0, 100, 30).await
    ));

    let dump = frames.join("\n\n");
    if let Ok(path) = std::env::var("ATUIN_RESUME_DUMP") {
        std::fs::write(path, &dump).unwrap();
    }
    assert!(!dump.is_empty());
}

#[rstest]
fn details_wait_while_the_selection_moves_fast() {
    use std::time::{Duration, Instant};

    use atuin_client::ai_session::HarnessKind;

    use super::{SETTLE, Settle};

    let handle = |id: &str| fake::row(HarnessKind::ClaudeCode, id, "t").handle;
    let mut settle = Settle::default();
    let t0 = Instant::now();
    // A single move asks at once.
    assert!(settle.ready(&handle("a"), t0));
    let t1 = t0 + SETTLE * 2;
    assert!(settle.ready(&handle("b"), t1));
    // Moving again straight away waits, and each further move pushes it back.
    let t2 = t1 + Duration::from_millis(10);
    assert!(!settle.ready(&handle("c"), t2));
    let t3 = t2 + Duration::from_millis(10);
    assert!(!settle.ready(&handle("d"), t3));
    assert_eq!(settle.due, Some(t3 + SETTLE));
    assert!(!settle.ready(&handle("d"), t3 + SETTLE / 2));
    // Once it settles, the session it stopped on is asked for.
    assert!(settle.ready(&handle("d"), t3 + SETTLE));
    assert_eq!(settle.due, None);
}

// --- continuing in another harness ---------------------------------------------------------------

fn press(state: &mut State, settings: &Settings, key: &str) -> super::state::InputAction {
    use atuin_client::tui::{KeyCodeValue, KeyInput, SingleKey};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let KeyInput::Single(SingleKey {
        code, ctrl, alt, ..
    }) = KeyInput::parse(key).unwrap()
    else {
        panic!("one key: {key}");
    };
    let code = match code {
        KeyCodeValue::Char(c) => KeyCode::Char(c),
        KeyCodeValue::Enter => KeyCode::Enter,
        KeyCodeValue::Esc => KeyCode::Esc,
        KeyCodeValue::Tab => KeyCode::Tab,
        KeyCodeValue::Up => KeyCode::Up,
        KeyCodeValue::Down => KeyCode::Down,
        other => panic!("{other:?}"),
    };
    let mut modifiers = KeyModifiers::NONE;
    if ctrl {
        modifiers |= KeyModifiers::CONTROL;
    }
    if alt {
        modifiers |= KeyModifiers::ALT;
    }
    state.handle_key_input(settings, &KeyEvent::new(code, modifiers))
}

/// `c` in the Inspect tab (alt-c anywhere) opens the chooser on the selected session: only the
/// other harnesses, and what continuing flattens. The keys move and pick in it, and nothing
/// reaches the query while it's open.
#[rstest]
#[tokio::test]
async fn the_chooser_offers_the_other_harnesses_and_picks_one() {
    use atuin_client::ai_session::HarnessKind;
    use atuin_common::harnesstools::continuation::Flattened;

    use super::state::{InputAction, Pending};

    let settings = settings();
    let mut state = loaded(&settings, "", 1).await;
    let row = state.selected().unwrap().clone();
    assert_eq!(row.handle.harness, HarnessKind::ClaudeCode);

    assert!(matches!(press(&mut state, &settings, "c"), InputAction::ChooseHarness(_)));
    assert!(state.open_chooser(FakeResumer::default().continue_targets(&row)));
    let flattened = Flattened {
        tool_calls: 42,
        tool_results: 42,
        reasoning: 3,
    };
    state.flattened.insert(row.handle, Ok(flattened));
    let out = text(&render(&mut state, &settings, 100, 30));
    assert!(out.contains("Continue this Claude Code session in"), "{out}");
    assert!(
        out.contains("1 CX Codex") && out.contains("2 OC opencode") && out.contains("3 PI Pi"),
        "{out}"
    );
    assert!(!out.contains("CC Claude Code"), "not its own harness: {out}");
    assert!(out.contains("42 tool calls flattened to notes, reasoning dropped"), "{out}");

    assert_eq!(press(&mut state, &settings, "down"), InputAction::Continue);
    assert_eq!(press(&mut state, &settings, "x"), InputAction::Continue);
    assert_eq!(
        press(&mut state, &settings, "enter"),
        InputAction::ContinueIn(HarnessKind::Opencode, Pending::Resume)
    );
    assert!(state.chooser.is_none());

    state.tab_index = 0;
    assert!(matches!(press(&mut state, &settings, "alt-c"), InputAction::ChooseHarness(_)));
    assert!(state.open_chooser(vec![HarnessKind::Codex, HarnessKind::Pi]));
    assert_eq!(
        press(&mut state, &settings, "tab"),
        InputAction::ContinueIn(HarnessKind::Codex, Pending::Edit)
    );
    assert!(state.open_chooser(vec![HarnessKind::Codex, HarnessKind::Pi]));
    assert_eq!(
        press(&mut state, &settings, "2"),
        InputAction::ContinueIn(HarnessKind::Pi, Pending::Resume)
    );
    assert!(state.open_chooser(vec![HarnessKind::Codex]));
    assert_eq!(press(&mut state, &settings, "esc"), InputAction::Continue);
    assert!(state.chooser.is_none());
    assert_eq!(state.input.as_str(), "", "no key reached the query");

    assert!(!state.open_chooser(Vec::new()));
    let (status, _) = state.status.clone().unwrap();
    assert!(status.contains("no other harness is installed"), "{status}");
}

/// A continuation written out ends the picker the way its key asked, leaving the status line
/// that says what was flattened; one that failed keeps the picker open, saying why.
#[rstest]
#[tokio::test]
async fn a_written_continuation_resumes_or_edits_its_new_session() {
    use atuin_client::ai_session::HarnessKind;

    use super::resumer::NotResumable;
    use super::state::Pending;
    use super::{Outcome, finish_continuation};

    let settings = settings();
    let mut state = loaded(&settings, "", 1).await;
    let row = state.selected().unwrap().clone();
    let source = FakeSource::new();
    let resumer = FakeResumer::default();

    for (action, run) in [(Pending::Resume, true), (Pending::Edit, false)] {
        state.continuing = Some((row.handle.clone(), HarnessKind::Codex, action));
        let continued = resumer.continue_in(&source, &row, HarnessKind::Codex).await;
        let (outcome, status) = finish_continuation(&mut state, &row.handle, continued).unwrap();
        assert_eq!(
            status,
            "continuing in Codex: 42 tool calls flattened to notes, reasoning dropped"
        );
        let plan = match (outcome, run) {
            (Outcome::Resume(plan), true) | (Outcome::Edit(plan), false) => plan,
            (other, _) => panic!("{other:?}"),
        };
        assert_eq!(plan.program, "codex");
        let id = format!("continued-{}", row.handle.session);
        assert_eq!(plan.args, ["resume", id.as_str()]);
    }

    state.continuing = Some((row.handle.clone(), HarnessKind::Pi, Pending::Resume));
    let failed = Err(NotResumable::NotInstalled("pi".to_owned()));
    assert!(finish_continuation(&mut state, &row.handle, failed).is_none());
    let (status, _) = state.status.clone().unwrap();
    assert_eq!(status, "can't continue in Pi: `pi` isn't installed here (not found on PATH)");
    // An answer for a continuation nobody is waiting on changes nothing.
    let stray = Err(NotResumable::Unsupported("x"));
    assert!(finish_continuation(&mut state, &row.handle, stray).is_none());
}

/// A session continued in another harness shows under the one it continues as a fork saying
/// where it went on.
#[rstest]
fn a_continuation_in_the_tree_says_where_it_went_on() {
    use atuin_client::ai_session::HarnessKind;

    use super::source::Relation;

    let root = fake::row(HarnessKind::ClaudeCode, "root", "fix the flaky test");
    let mut child = fake::row(HarnessKind::Codex, "0199aaaa", "fix the flaky test");
    child.parent = Some(root.handle.clone());
    child.relation = Relation::Fork;
    let mut themes = ThemeManager::new(None, None);
    let theme = themes.load_theme("default", None);
    let lines = super::panel::tree_lines(&root.handle, &[child], fake::now(), 90, theme);
    let line: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        line.contains("fork") && line.contains("continued in Codex · fix the flaky test"),
        "{line}"
    );
}
