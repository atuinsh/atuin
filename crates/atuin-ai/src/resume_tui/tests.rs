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
/// from sync, behind the scenes (the row doesn't say so).
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
    assert!(!out.contains("from sync"), "{out}");
}

/// Inspecting another host's session says how it resumes, hinting that it comes from sync.
#[rstest]
#[tokio::test]
async fn inspect_says_a_remote_session_is_restored() {
    let mut s = settings();
    s.ai.sessions.filter_mode = Some(atuin_client::settings::AiSessionFilterMode::Global);
    let mut state = loaded(&s, "aarch64", 1).await;
    let out = text(&render(&mut state, &s, 120, 30));
    let line = out.lines().find(|l| l.contains("Resume    cd -- ")).unwrap();
    assert!(line.contains(" && claude --resume d4e6f8a0-2c3d-4e4f-8a7b-8c9d0e1f2a3b  from sync"));
    assert!(!out.contains("Restore"), "{out}");
}

#[rstest]
#[tokio::test]
async fn preview_shows_first_prompt_match_and_last_reply() {
    let out = frame(&settings(), "subagents", 0, 100, 30).await;
    assert!(out.contains("first  Build atuin ai resume: a picker"), "{out}");
    assert!(out.contains("match  …Group forks and subagents under their root session"), "{out}");
    assert!(out.contains("last   Grouping done Rows now fold forks and subagents"), "{out}");

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

/// The frame's lines between the rows starting `from` and `to` (exclusive), borders stripped.
fn section(out: &str, from: &str, to: &str) -> Vec<String> {
    out.lines()
        .map(|l| {
            let l = l.trim_start();
            l.strip_prefix('│').unwrap_or(l).trim_end().trim_end_matches('│').trim_end().to_owned()
        })
        .skip_while(|l| !l.trim_start().starts_with(from))
        .take_while(|l| !l.trim_start().starts_with(to) || l.trim_start().starts_with(from))
        .collect()
}

fn markdown_settings(lines: u16) -> Settings {
    let mut s = settings();
    s.max_preview_height = lines;
    s.preview.strategy = atuin_client::settings::PreviewStrategy::Fixed;
    s
}

#[rstest]
#[case::wide(100)]
#[case::narrow(64)]
#[tokio::test]
async fn preview_renders_markdown(#[case] width: u16) {
    let s = markdown_settings(14);
    let out = frame(&s, "", 0, width, 40).await;
    let preview = section(&out, "first", "╰");
    let joined = preview.join("\n");
    // No raw markup left.
    for raw in ["**", "```", "## ", "`enter`", "|:--", "](http"] {
        assert!(!joined.contains(raw), "{raw:?} in\n{joined}");
    }
    // Lists hang under their bullets; code is indented; the table lines up.
    assert!(joined.contains("       • resume on enter, edit on tab"), "{joined}");
    assert!(joined.contains("         row.children = u32::try_from("), "{joined}");
    assert!(joined.contains("Harness     │ Forks │ Subagents"), "{joined}");
    assert!(joined.contains("Codex       │     0 │         0"), "{joined}");
    // Everything fits inside the box.
    let inner = usize::from(width) - 4;
    for line in &preview {
        assert!(line.chars().count() <= inner, "{line:?}");
    }
}

#[rstest]
#[tokio::test]
async fn markdown_styles_come_from_the_theme() {
    let s = markdown_settings(14);
    let mut state = loaded(&s, "", 0).await;
    let buf = render(&mut state, &s, 100, 40);
    let out = text(&buf);
    let find = |needle: &str| {
        let (y, line) = out.lines().enumerate().find(|(_, l)| l.contains(needle)).unwrap();
        let x = line[..line.find(needle).unwrap()].chars().count();
        buf[(u16::try_from(x).unwrap(), u16::try_from(y).unwrap())].clone()
    };
    let bold = ratatui::style::Modifier::BOLD;
    assert!(find("search.").modifier.contains(bold), "strong");
    assert!(find("Grouping done").modifier.contains(bold), "heading");
    // Inline code and code blocks in the theme's command colour; bullets muted.
    assert_eq!(find("atuin ai resume:").fg, ratatui::style::Color::LightGreen);
    assert_eq!(find("row.children").fg, ratatui::style::Color::LightGreen);
    assert_eq!(find("• a preview").fg, ratatui::style::Color::DarkGray);
}

#[rstest]
#[tokio::test]
async fn highlights_show_through_the_markdown() {
    let s = markdown_settings(10);
    let mut state = loaded(&s, "wall", 0).await;
    let buf = render(&mut state, &s, 100, 30);
    let out = text(&buf);
    // The match is in the last reply, inside `**wall-clock ordering**`.
    let (y, line) = out.lines().enumerate().find(|(_, l)| l.contains("last   ")).unwrap();
    assert!(line.contains("It depends on wall-clock ordering:"), "{out}");
    let at = |needle: &str| {
        let x = line[..line.find(needle).unwrap()].chars().count();
        buf[(u16::try_from(x).unwrap(), u16::try_from(y).unwrap())].clone()
    };
    let warn = ratatui::style::Color::Yellow;
    assert_eq!(at("wall").fg, warn, "highlighted");
    assert!(at("wall").modifier.contains(ratatui::style::Modifier::BOLD));
    assert_ne!(at("clock").fg, warn, "only the match");
    assert!(at("clock").modifier.contains(ratatui::style::Modifier::BOLD), "still strong");
    assert_ne!(at("depends").fg, warn);
}

#[rstest]
#[tokio::test]
async fn long_replies_are_cut_with_an_ellipsis() {
    let s = markdown_settings(3);
    let out = frame(&s, "", 0, 100, 30).await;
    let preview = section(&out, "first", "╰");
    assert_eq!(preview.len(), 3, "{out}");
    // Two lines for the prompt, cut after its first paragraph; the reply gets one, its heading
    // and text run on and cut.
    assert!(preview[1].ends_with("search.…"), "{preview:#?}");
    assert!(preview[2].starts_with("last   Grouping done Rows now fold"), "{preview:#?}");
    assert!(preview[2].ends_with('…'), "{preview:#?}");
}

#[rstest]
#[tokio::test]
async fn the_detail_pane_and_inspect_render_markdown() {
    let out = frame(&settings(), "", 0, 150, 50).await;
    assert!(out.contains("│ • a preview pane with the first prompt and the last"), "{out}");
    assert!(out.contains("Harness     │ Forks │ Subagents"), "{out}");
    assert!(out.contains("────────────┼───────┼──────────"), "{out}");
    assert!(out.contains("Next up (see the design notes"), "{out}");
    assert!(out.contains("(https://docs.atuin.sh/ai/resume)):"), "{out}");
    assert!(out.contains("│ Tool calls and reasoning stay out of the preview."), "{out}");

    let out = frame(&settings(), "", 1, 100, 50).await;
    let conversation = section(&out, "First prompt", "<esc>");
    let joined = conversation.join("\n");
    assert!(joined.contains("\n • resume on enter, edit on tab"), "{joined}");
    assert!(joined.contains(" Last reply\n Grouping done"), "{joined}");
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
    use super::state::Pending;

    let s = settings();
    let mut frames = Vec::new();
    let mut compact = s.clone();
    compact.style = UiStyle::Compact;
    let tall = markdown_settings(14);
    for (label, settings, query, tab, w, h) in [
        ("fixed preview 14, 100x40", &tall, "", 0, 100, 40),
        ("fixed preview 14, 64x40", &tall, "", 0, 64, 40),
        ("fixed preview 10, query 'wall', 80x30", &markdown_settings(10), "wall", 0, 80, 30),
        ("full, wide split, 150x50", &s, "", 0, 150, 50),
        ("full, inspect (ctrl-o), 100x50", &s, "", 1, 100, 50),
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

    for (label, settings, query, action, w, h) in [
        ("chooser (enter), 100x30", &s, "", Pending::Resume, 100, 30),
        ("chooser (tab), 100x30", &s, "", Pending::Edit, 100, 30),
        ("chooser, another host's session, 100x30", &global, "aarch64", Pending::Resume, 100, 30),
        ("chooser, directory gone, 100x30", &global, "theme preview", Pending::Resume, 100, 30),
        ("chooser, wide split, 140x32", &s, "", Pending::Resume, 140, 32),
        ("chooser, 80x14 (inline_height = 14)", &s, "", Pending::Resume, 80, 14),
    ] {
        let mut state = with_chooser(settings, query, action).await;
        let frame = text(&render(&mut state, settings, w, h));
        frames.push(format!("=== {label} ===\n{frame}"));
    }
    let mut inverted = s.clone();
    inverted.invert = true;
    let mut state = with_chooser(&inverted, "", Pending::Resume).await;
    let frame = text(&render(&mut state, &inverted, 100, 30));
    frames.push(format!("=== chooser, inverted, 100x30 ===\n{frame}"));

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

fn chooser_settings() -> Settings {
    let mut s = settings();
    s.ai.sessions.filter_mode = Some(atuin_client::settings::AiSessionFilterMode::Global);
    s
}

/// The selected session's chooser, opened by `action`, with what continuing flattens read.
async fn with_chooser(settings: &Settings, query: &str, action: super::state::Pending) -> State {
    use atuin_common::harnesstools::continuation::Flattened;

    let mut state = loaded(settings, query, 0).await;
    let row = state.selected().unwrap().clone();
    state.open_chooser(FakeResumer::default().continue_targets(&row), action);
    let flattened = Flattened {
        tool_calls: 42,
        tool_results: 42,
        reasoning: 3,
    };
    state.flattened.insert(row.handle, Ok(flattened));
    state
}

/// Enter on a session asks where to resume it: its own harness first and selected, then the
/// other harnesses installed here, saying what continuing there flattens. Enter picks the
/// selected line the way the key that opened it asked (resume, with `enter_accept`), tab edits,
/// a digit picks its line, and nothing reaches the query while it's open.
#[rstest]
#[tokio::test]
async fn the_chooser_offers_the_original_first_then_the_others() {
    use atuin_client::ai_session::HarnessKind;

    use super::state::{InputAction, Pending};

    let s = settings();
    let mut state = with_chooser(&s, "", Pending::Resume).await;
    let out = text(&render(&mut state, &s, 100, 30));
    assert!(out.contains("╭ Resume in "), "{out}");
    assert!(out.contains("> 1 CC Claude Code  original"), "{out}");
    assert!(
        out.contains("  2 CX Codex        continue, 42 tool calls become notes, reasoning dropped")
    );
    assert!(out.contains("  3 OC opencode     continue, 42 tool calls"), "{out}");
    assert!(out.contains("  4 PI Pi           continue, 42 tool calls"), "{out}");
    assert!(out.contains("<enter>: resume  <tab>: edit  <esc>: back"), "{out}");
    // It opens over the list, against the selected row, which stays in sight below it.
    let lines: Vec<&str> = out.lines().collect();
    let bottom = lines.iter().position(|l| l.contains('╰') && l.contains("──╯")).unwrap();
    assert!(lines[bottom + 1].contains(" > "), "{out}");

    assert_eq!(press(&mut state, &s, "x"), InputAction::Continue);
    assert_eq!(press(&mut state, &s, "enter"), InputAction::Pick(None, Pending::Resume));
    assert!(state.chooser.is_none() && state.accept);
    assert_eq!(state.input.as_str(), "", "no key reached the query");

    let mut state = with_chooser(&s, "", Pending::Resume).await;
    assert_eq!(press(&mut state, &s, "down"), InputAction::Continue);
    assert_eq!(press(&mut state, &s, "j"), InputAction::Continue);
    assert_eq!(press(&mut state, &s, "k"), InputAction::Continue);
    assert_eq!(
        press(&mut state, &s, "tab"),
        InputAction::Pick(Some(HarnessKind::Codex), Pending::Edit)
    );

    let mut state = with_chooser(&s, "", Pending::Resume).await;
    assert_eq!(press(&mut state, &s, "9"), InputAction::Continue, "no ninth line");
    assert_eq!(
        press(&mut state, &s, "4"),
        InputAction::Pick(Some(HarnessKind::Pi), Pending::Resume)
    );

    let mut state = with_chooser(&s, "", Pending::Resume).await;
    assert_eq!(press(&mut state, &s, "esc"), InputAction::Continue);
    assert!(state.chooser.is_none(), "esc goes back to the list");
}

/// Opened with tab (or enter without `enter_accept`), the chooser edits whichever line is picked,
/// and says so.
#[rstest]
#[tokio::test]
async fn a_chooser_opened_to_edit_edits() {
    use atuin_client::ai_session::HarnessKind;

    use super::state::{InputAction, Pending};

    let s = settings();
    let mut state = with_chooser(&s, "", Pending::Edit).await;
    let out = text(&render(&mut state, &s, 100, 30));
    assert!(out.contains("<enter>: edit  <esc>: back"), "{out}");
    assert_eq!(press(&mut state, &s, "enter"), InputAction::Pick(None, Pending::Edit));
    assert!(!state.accept);
    let mut state = with_chooser(&s, "", Pending::Edit).await;
    assert_eq!(
        press(&mut state, &s, "2"),
        InputAction::Pick(Some(HarnessKind::Codex), Pending::Edit)
    );
}

/// A session restored from sync says so only as a hint on its own harness's line.
#[rstest]
#[tokio::test]
async fn a_session_from_another_host_hints_it_comes_from_sync() {
    use super::state::Pending;

    let s = chooser_settings();
    let mut state = with_chooser(&s, "aarch64", Pending::Resume).await;
    let out = text(&render(&mut state, &s, 100, 30));
    assert!(out.contains("> 1 CC Claude Code  original, from sync"), "{out}");
    assert!(out.contains("  2 CX Codex        continue"), "{out}");
}

/// A session its own harness can't resume here (a subagent, a deleted directory, a harness
/// that isn't installed) shows why, dimmed, and the first line that works is selected instead;
/// also once the plan saying so comes in after the chooser opened, unless the selection was
/// moved. Picking it anyway says why and stays open.
#[rstest]
#[tokio::test]
async fn an_original_that_cant_resume_is_dimmed_and_passed_over() {
    use atuin_client::ai_session::HarnessKind;
    use atuin_common::harnesstools::resume::ResumeError;

    use super::resumer::NotResumable;
    use super::state::{InputAction, Pending};

    let s = chooser_settings();
    let mut state = loaded(&s, "theme preview", 0).await;
    let row = state.selected().unwrap().clone();
    let why = state.original_unavailable(&row.handle).cloned().expect("its directory is gone");
    assert!(matches!(why, NotResumable::Harness(ResumeError::CwdMissing(_))), "{why:?}");
    let targets = FakeResumer::default().continue_targets(&row);
    state.open_chooser(targets.clone(), Pending::Resume);
    let buf = render(&mut state, &s, 100, 30);
    let out = text(&buf);
    let (y, line) = out.lines().enumerate().find(|(_, l)| l.contains("1 PI Pi")).unwrap();
    assert!(line.contains("Pi           original: the session's directory is gone"), "{out}");
    let x = u16::try_from(line[..line.find("Pi ").unwrap()].chars().count()).unwrap();
    let cell = &buf[(x, u16::try_from(y).unwrap())];
    assert!(cell.modifier.contains(ratatui::style::Modifier::DIM), "dimmed");
    assert!(out.contains("> 2 CC Claude Code"), "the first line that works is selected: {out}");

    assert_eq!(press(&mut state, &s, "1"), InputAction::Continue);
    let (status, _) = state.status.clone().unwrap();
    assert!(status.starts_with("can't resume in Pi: the session's directory is gone"), "{status}");
    assert!(state.chooser.is_some(), "stays open");

    // The plan comes in after the chooser opened: the selection moves off, unless it was moved.
    let plan = state.plans.remove(&row.handle).unwrap();
    for (moved, selected) in [(false, 1), (true, 0)] {
        state.open_chooser(targets.clone(), Pending::Resume);
        assert_eq!(state.chooser.as_ref().unwrap().selected, 0);
        if moved {
            press(&mut state, &s, "down");
            press(&mut state, &s, "up");
        }
        state.plans.insert(row.handle.clone(), plan.clone());
        state.settle_chooser();
        assert_eq!(state.chooser.as_ref().unwrap().selected, selected, "moved: {moved}");
        state.plans.remove(&row.handle);
    }

    // A harness atuin can't continue from (Copilot) has only its own line, saying why.
    let mut copilot = fake::row(HarnessKind::Copilot, "cp1", "t");
    copilot.host_id = fake::THIS_HOST_ID.to_owned();
    state.results = vec![copilot.clone()];
    state.list.selected = 0;
    state.plans.insert(copilot.handle.clone(), FakeResumer::default().plan(&copilot).await);
    state.open_chooser(FakeResumer::default().continue_targets(&copilot), Pending::Resume);
    let out = text(&render(&mut state, &s, 100, 30));
    assert!(out.contains("> 1 CP Copilot  original: atuin can't resume"), "{out}");
}

/// Enter asks where to resume only when there is a choice: with `resume_chooser = false`, or
/// nothing else installed, it resumes in the session's own harness straight away. When that
/// can't, the chooser opens anyway, saying why.
#[rstest]
#[tokio::test]
async fn the_chooser_opens_when_there_is_a_choice() {
    use std::sync::Arc;

    use super::state::Pending;
    use super::{Outcome, accept, worker};

    let s = chooser_settings();
    let resumer = Arc::new(FakeResumer::default());
    let (requests, _responses) = worker::spawn(Arc::new(FakeSource::new()), resumer.clone());

    let mut state = loaded(&s, "", 0).await;
    let outcome = accept(&mut state, Pending::Resume, resumer.as_ref(), &requests, true);
    assert_eq!(outcome, None);
    assert!(state.chooser.is_some());

    let mut state = loaded(&s, "", 0).await;
    let outcome = accept(&mut state, Pending::Resume, resumer.as_ref(), &requests, false);
    assert!(matches!(outcome, Some(Outcome::Resume(_))), "{outcome:?}");
    let mut state = loaded(&s, "", 0).await;
    let outcome = accept(&mut state, Pending::Edit, resumer.as_ref(), &requests, false);
    assert!(matches!(outcome, Some(Outcome::Edit(_))), "{outcome:?}");

    // ctrl-y copies the original's command; it never asks.
    let mut state = loaded(&s, "", 0).await;
    let outcome = accept(&mut state, Pending::Copy, resumer.as_ref(), &requests, true);
    assert_eq!(outcome, None);
    assert!(state.chooser.is_none());

    let mut state = loaded(&s, "theme preview", 0).await;
    let outcome = accept(&mut state, Pending::Resume, resumer.as_ref(), &requests, false);
    assert_eq!(outcome, None);
    let chooser = state.chooser.as_ref().expect("opens anyway");
    assert_eq!(chooser.selected, 1);
    let (status, _) = state.status.clone().unwrap();
    assert!(status.contains("the session's directory is gone"), "{status}");
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

/// The picker's host id is compared with the rows' in their (simple) form, however it was
/// given: a hyphenated one would make every session of this host look like another host's.
#[rstest]
#[case::hyphenated("01a0e0e0-9cdc-763b-9734-7b45cb98e831")]
#[case::simple("01a0e0e09cdc763b97347b45cb98e831")]
#[case::upper("01A0E0E0-9CDC-763B-9734-7B45CB98E831")]
fn host_ids_compare_in_one_form(#[case] id: &str) {
    assert_eq!(super::simple_host_id(id), "01a0e0e09cdc763b97347b45cb98e831");
    assert_eq!(super::simple_host_id("not-a-uuid"), "not-a-uuid");
}
