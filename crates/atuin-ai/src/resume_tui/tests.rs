//! Rendering tests: the picker drawn from the fake source into ratatui's `TestBackend`.

use atuin_client::settings::{KeymapMode, Settings, Style as UiStyle};
use atuin_client::theme::ThemeManager;
use atuin_client::tui::cursor::Cursor;
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
        let children = source.children(&row.handle).await.unwrap();
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
    assert!(out.contains("[  WORKSPACE 10  ]"), "the count beside the mode: {out}");
    assert!(out.lines().last().unwrap().trim_start().starts_with('╰'), "{out}");
}

/// Rows are the time, the harness, the title and the message count: nothing about where.
#[rstest]
#[tokio::test]
async fn rows_show_time_badge_title_and_count() {
    let out = frame(&settings(), "", 0, 100, 30).await;
    // The newest session sits at the bottom (not inverted), selected.
    let selected = out.lines().find(|l| l.contains(" > ")).unwrap();
    assert!(selected.contains("● now CC Add an interactive resume picker"), "{selected}");
    assert!(selected.trim_end_matches(['│', ' ']).ends_with("142"), "{selected}");
    // No `+N`, repository or branch on the row.
    assert!(!selected.contains("+4") && !selected.contains("+1"), "{selected}");
    assert!(!selected.contains("ai-resume"), "{selected}");
    // Workspace hides the dotfiles and remote sessions.
    assert!(!out.contains("dotfiles"), "{out}");
    // Subagents are never listed.
    assert!(!out.contains("Explore: find"), "{out}");
}

/// Other hosts' sessions look like this host's: they resume by being restored from sync,
/// behind the scenes. Only the preview says where one ran.
#[rstest]
#[tokio::test]
async fn other_hosts_rows_look_like_this_hosts() {
    let mut state = loaded(&settings(), "", 0).await;
    state.mode = atuin_client::settings::AiSessionFilterMode::Global;
    let source = FakeSource::new();
    let (generation, mode, filter) = state.next_search().unwrap();
    state.apply_results(generation, mode, source.search(&filter).await.unwrap());
    let buf = render(&mut state, &settings(), 100, 30);
    let out = text(&buf);
    assert!(!out.contains('@'), "no host on any row: {out}");
    let (y, line) =
        out.lines().enumerate().find(|(_, l)| l.contains("Bisect the aarch64")).unwrap();
    let x = u16::try_from(line.find("Bisect").unwrap()).unwrap();
    let cell = &buf[(x, u16::try_from(y).unwrap())];
    assert!(!cell.modifier.contains(ratatui::style::Modifier::DIM), "remote rows aren't dimmed");

    let remote = state.results.iter().position(|r| r.host_id != fake::THIS_HOST_ID).unwrap();
    state.list.selected = remote;
    let out = text(&render(&mut state, &settings(), 100, 30));
    assert!(out.contains("│       atuin · main · @00000002"), "{out}");
    assert!(!out.contains("from sync"), "{out}");
}

/// The preview's first line says where the session ran and what forked off it, in place of a
/// line of text, leaving out this host and a detached branch; with no room for it, the text
/// keeps the line.
#[rstest]
#[tokio::test]
async fn the_preview_says_where_a_session_ran() {
    let s = markdown_settings(4);
    let out = frame(&s, "", 0, 100, 30).await;
    let preview = section(&out, "atuin", "╰");
    assert_eq!(preview.len(), 4, "{out}");
    assert_eq!(preview[0], "       atuin · ai-resume · 1 fork", "{out}");
    assert!(preview[1].starts_with("first  "), "{out}");

    let out = frame(&markdown_settings(1), "", 0, 100, 30).await;
    assert!(!out.contains("atuin · ai-resume"), "{out}");
    assert!(out.contains("first  "), "{out}");

    // Nothing grouped under it: no count.
    let out = frame(&s, "flaky", 0, 100, 30).await;
    assert_eq!(section(&out, "atuin", "╰")[0], "       atuin · main", "{out}");
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
#[case("agent:codex")]
#[case("a:codex")]
#[tokio::test]
async fn tokens_render_as_chips(#[case] token: &str) {
    let mut state = loaded(&settings(), &format!("{token} flaky"), 0).await;
    let buf = render(&mut state, &settings(), 100, 30);
    let out = text(&buf);
    let (y, line) = out.lines().enumerate().find(|(_, l)| l.contains(token)).unwrap();
    let x = u16::try_from(line[..line.find(token).unwrap()].chars().count()).unwrap();
    let y = u16::try_from(y).unwrap();
    assert!(buf[(x, y)].modifier.contains(ratatui::style::Modifier::REVERSED));
    let plain = x + u16::try_from(token.len() + 1).unwrap();
    assert!(!buf[(plain, y)].modifier.contains(ratatui::style::Modifier::REVERSED));
    assert!(out.contains("Fix the flaky sync test"));
    assert!(!out.contains("Add an interactive"));
}

/// The frame's lines between the rows starting `from` and `to` (exclusive), borders stripped.
fn section(out: &str, from: &str, to: &str) -> Vec<String> {
    out.lines()
        .map(|l| {
            let l = l.trim_start();
            // The right border, or a scrollbar's track and thumb on it.
            l.strip_prefix('│')
                .unwrap_or(l)
                .trim_end()
                .trim_end_matches(['│', '┃'])
                .trim_end()
                .to_owned()
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
    assert!(out.contains("Host      01900000000070008000000000000001  (this machine)"), "{out}");
    assert!(out.contains(
        "Resume    cd -- /home/ellie/src/atuin && claude --resume \
         7f3c9a12-5be0-4d7e-9c41-0a8e2b6f4d10"
    ));
    // The forks, never the subagents (three under the session, one under its fork).
    assert!(out.lines().any(|l| l.trim_matches(['│', ' ']) == "1 fork"), "{out}");
    assert!(out.contains("   └─ Add an interactive resume picker to atuin ai (fork)"), "{out}");
    assert!(!out.contains("subagent  ") && !out.contains("Explore:"), "{out}");
    assert!(!out.contains("Review the resume picker diff"), "{out}");
    assert!(out.lines().any(|l| l.trim_matches(['│', ' ']) == "Messages  142"), "{out}");
    // All the input, and what it was made of.
    assert!(
        out.contains(
            "Tokens    in 3.2M (327k uncached · 2.7M cache read · 170k cache write) · out 58k"
        ),
        "{out}"
    );
    assert!(!out.contains("Activity"), "no activity chart: {out}");
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
    assert!(out.contains("[  WORKSPACE 10  ]"), "{out}");

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
    assert!(out.contains("[  WS→GLOBAL 14  ]"), "{out}");
    assert!(out.contains("Bisect the aarch64"), "{out}");
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

fn press(state: &mut State, settings: &Settings, key: &str) -> super::state::InputAction {
    use atuin_client::tui::key::{KeyCodeValue, KeyInput, SingleKey};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let KeyInput::Single(SingleKey {
        code,
        ctrl,
        alt,
        shift,
        ..
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
        KeyCodeValue::PageUp => KeyCode::PageUp,
        KeyCodeValue::PageDown => KeyCode::PageDown,
        other => panic!("{other:?}"),
    };
    let mut modifiers = KeyModifiers::NONE;
    if ctrl {
        modifiers |= KeyModifiers::CONTROL;
    }
    if alt {
        modifiers |= KeyModifiers::ALT;
    }
    if shift {
        modifiers |= KeyModifiers::SHIFT;
    }
    state.handle_key_input(settings, &KeyEvent::new(code, modifiers))
}

/// Enter and tab resume the session straight away, once its plan is known; ctrl-y copies the
/// command. One that can't be resumed here says why, and the picker stays open.
#[rstest]
#[tokio::test]
async fn enter_and_tab_resume_straight_away() {
    use std::sync::Arc;

    use super::state::Pending;
    use super::{Outcome, accept, worker};

    let mut s = settings();
    s.ai.sessions.filter_mode = Some(atuin_client::settings::AiSessionFilterMode::Global);
    let resumer = Arc::new(FakeResumer::default());
    let (requests, _responses) = worker::spawn(Arc::new(FakeSource::new()), resumer.clone());

    let mut state = loaded(&s, "", 0).await;
    let outcome = accept(&mut state, Pending::Resume, &requests);
    assert!(matches!(outcome, Some(Outcome::Resume(_))), "{outcome:?}");
    let mut state = loaded(&s, "", 0).await;
    let outcome = accept(&mut state, Pending::Edit, &requests);
    assert!(matches!(outcome, Some(Outcome::Edit(_))), "{outcome:?}");

    // Its directory is gone, and pi can't resume it from anywhere else.
    let mut state = loaded(&s, "theme preview", 0).await;
    assert_eq!(accept(&mut state, Pending::Resume, &requests), None);
    let (status, _) = state.status.clone().unwrap();
    assert!(status.starts_with("can't resume: "), "{status}");
}

/// A session recorded on another machine is restored from sync (by the worker) only once it is
/// chosen, and then resumed from where it was written; ctrl-y copies `atuin ai resume <id>`,
/// which restores it when run, and writes nothing.
#[rstest]
#[tokio::test]
async fn a_session_from_another_machine_is_restored_first() {
    use std::sync::Arc;

    use super::state::{Pending, RESTORE};
    use super::worker::Response;
    use super::{Outcome, accept, apply_response, resume_line, worker};

    let mut s = settings();
    s.ai.sessions.filter_mode = Some(atuin_client::settings::AiSessionFilterMode::Global);
    let resumer = Arc::new(FakeResumer::default());
    let (requests, _responses) = worker::spawn(Arc::new(FakeSource::new()), resumer.clone());

    let mut state = loaded(&s, "aarch64 release build", 0).await;
    let row = state.selected().unwrap().clone();
    assert_ne!(row.host_id, fake::THIS_HOST_ID, "another machine's session");
    let resume = state.plans[&row.handle].clone().unwrap();
    assert!(resume.restore.is_some());
    let id = row.handle.session.to_string();
    assert_eq!(resume_line(&row, &resume), format!("atuin ai resume {id}"));

    let outcome = accept(&mut state, Pending::Resume, &requests);
    assert_eq!(outcome, None, "waits for the restore");
    assert!(state.requested.contains(&(row.handle.clone(), RESTORE)));
    assert!(state.status.as_ref().is_some_and(|(s, _)| s.starts_with("restoring from sync")));

    let restore = resume.restore.clone().unwrap();
    let plan = resumer.restore(&FakeSource::new(), &row, &restore).await;
    apply_response(&mut state, Response::Restored(row.handle.clone(), plan), &requests);
    let outcome = accept(&mut state, Pending::Resume, &requests);
    let Some(Outcome::Resume(plan)) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(plan.native_path, Some(std::path::PathBuf::from(format!("/restored/{id}.jsonl"))));
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

// --- the UX pass ---------------------------------------------------------------------------------

/// `n` forks under the selected session.
fn many_children(state: &mut State, n: usize) {
    use atuin_client::ai_session::HarnessKind;

    use super::source::Relation;

    let root = state.selected().unwrap().handle.clone();
    let children = (0..n)
        .map(|i| {
            let mut child = fake::row(HarnessKind::ClaudeCode, &format!("fork-{i:02}"), "t");
            child.title = super::source::Snippet::plain(format!("Fork number {i}"));
            child.parent = Some(root.clone());
            child.relation = Relation::Fork;
            child
        })
        .collect();
    state.children.insert(root, children);
}

/// Inspect shows a few forks and says how many more, leaving the room to the conversation;
/// `c` expands them into a list the arrows move in (not between sessions), and esc collapses it.
#[rstest]
#[tokio::test]
async fn inspect_collapses_a_long_children_list() {
    let s = settings();
    let mut state = loaded(&s, "", 1).await;
    many_children(&mut state, 17);
    let out = text(&render(&mut state, &s, 100, 34));
    assert!(out.contains(" 17 forks"), "{out}");
    assert!(out.contains("Fork number 2"), "{out}");
    assert!(!out.contains("Fork number 3"), "{out}");
    assert!(out.contains("   … and 14 more (c to expand)"), "{out}");
    assert!(out.contains("First prompt") && out.contains("Last reply"), "{out}");

    let selected = state.list.selected;
    press(&mut state, &s, "c");
    let out = text(&render(&mut state, &s, 100, 34));
    assert!(out.contains("17 forks  1–"), "a scroll position: {out}");
    assert!(out.contains("<↑/↓>: move  <c>/<esc>: collapse"), "{out}");
    for _ in 0..16 {
        press(&mut state, &s, "down");
    }
    assert_eq!(state.list.selected, selected, "the arrows stay in the list");
    let buf = render(&mut state, &s, 100, 34);
    let out = text(&buf);
    assert!(out.contains("Fork number 16"), "scrolled to the cursor: {out}");
    let reversed = |needle: &str| {
        let (y, line) = out.lines().enumerate().find(|(_, l)| l.contains(needle)).unwrap();
        let x = line[..line.find(needle).unwrap()].chars().count();
        buf[(u16::try_from(x).unwrap(), u16::try_from(y).unwrap())]
            .modifier
            .contains(ratatui::style::Modifier::REVERSED)
    };
    assert!(reversed("Fork number 16"), "the cursor's row: {out}");
    assert!(!reversed("Fork number 15"), "{out}");
    assert!(out.contains("of 17"), "{out}");

    press(&mut state, &s, "esc");
    assert_eq!(state.tab_index, 1, "esc collapses first");
    assert!(state.expanded_children().is_none());
    press(&mut state, &s, "up");
    assert_ne!(state.list.selected, selected, "collapsed, the arrows move between sessions");
    press(&mut state, &s, "esc");
    assert_eq!(state.tab_index, 0);
}

/// With the forks expanded in Inspect, enter, tab and ctrl-y act on the fork highlighted (in the
/// tree's order, not the order the forks were read in), not on the session inspected.
#[rstest]
#[tokio::test]
async fn enter_and_tab_in_the_expanded_forks_resume_the_fork() {
    use std::sync::Arc;

    use super::state::{InputAction, Pending};
    use super::{Outcome, accept, worker};

    let s = settings();
    let resumer = Arc::new(FakeResumer::default());
    let (requests, _responses) = worker::spawn(Arc::new(FakeSource::new()), resumer.clone());
    let mut state = loaded(&s, "", 1).await;
    let root = state.selected().unwrap().clone();
    many_children(&mut state, 2);
    // fork-00 forked from fork-01, so the tree lists fork-01 first, and fork-00 under it.
    let children = state.children.get_mut(&root.handle).unwrap();
    children[0].parent = Some(children[1].handle.clone());
    for child in state.children[&root.handle].clone() {
        state.plans.insert(child.handle.clone(), resumer.plan(&child).await);
    }
    let resumes = |state: &mut State, action| match accept(state, action, &requests) {
        Some(Outcome::Resume(plan) | Outcome::Edit(plan)) => plan.args.join(" "),
        other => panic!("{other:?}"),
    };

    assert_eq!(state.target(), Some(&root), "collapsed, it's the session inspected");
    assert!(resumes(&mut state, Pending::Resume).contains(root.handle.session.as_ref()));

    press(&mut state, &s, "c");
    assert_eq!(state.target().unwrap().handle.session.as_ref(), "fork-01");
    press(&mut state, &s, "down");
    assert_eq!(state.target().unwrap().handle.session.as_ref(), "fork-00");
    assert_eq!(press(&mut state, &s, "enter"), InputAction::Resume);
    assert!(resumes(&mut state, Pending::Resume).ends_with("fork-00"));
    assert_eq!(press(&mut state, &s, "tab"), InputAction::ReturnCommand);
    assert!(resumes(&mut state, Pending::Edit).ends_with("fork-00"));
    assert_eq!(state.selected(), Some(&root), "the selection stays on the session inspected");

    press(&mut state, &s, "esc");
    assert_eq!(state.target(), Some(&root));
}

/// Rows never overflow and the selected title keeps its room, at every width, in the modes that
/// show the most and the least; the split layout included.
#[rstest]
#[tokio::test]
async fn rows_fit_every_width(#[values(false, true)] global: bool) {
    let mut s = settings();
    if global {
        s.ai.sessions.filter_mode = Some(atuin_client::settings::AiSessionFilterMode::Global);
    }
    let mut state = loaded(&s, "", 0).await;
    let title = state.selected().unwrap().title.text.clone();
    for width in 60..=200u16 {
        let out = text(&render(&mut state, &s, width, 30));
        let selected = out.lines().find(|l| l.contains(" > ")).unwrap();
        // The list's part of the row, before the divider of a split layout.
        let row = selected.trim_start_matches([' ', '│']).split('│').next().unwrap();
        let shown = title.chars().take(27).collect::<String>();
        assert!(row.contains(&shown), "{width}: {row:?}");
        if global {
            // Another host's row is no different.
            let remote = out.lines().find(|l| l.contains("Bisect the aarch64")).unwrap();
            assert!(!remote.contains("@00000002"), "{width}: {remote}");
        }
    }
}

/// The header and the mode prefix say when the search stopped at its limit.
#[rstest]
#[tokio::test]
async fn a_capped_list_says_so() {
    use atuin_client::ai_session::HarnessKind;

    use super::state::SEARCH_LIMIT;

    let s = settings();
    let mut state = loaded(&s, "", 0).await;
    state.results =
        (0..SEARCH_LIMIT).map(|i| fake::row(HarnessKind::Codex, &format!("s{i}"), "t")).collect();
    let out = text(&render(&mut state, &s, 100, 30));
    assert!(out.lines().next().unwrap().ends_with("500+ sessions"), "{out}");
    assert!(out.contains("[ WORKSPACE 500+ ]"), "{out}");
}

/// The detail pane counts the forks (not the subagents), and all the input tokens, with the
/// share read from the cache.
#[rstest]
#[tokio::test]
async fn the_detail_pane_counts_the_forks() {
    let out = frame(&settings(), "", 0, 150, 40).await;
    assert!(out.contains("142 messages · 1 fork · started"), "{out}");
    // No activity chart.
    assert!(!out.contains("over 3h") && !out.contains('█'), "{out}");
    assert!(out.contains("in 3.2M (84% cached) · out 58k tokens"), "{out}");
    // This host goes without saying.
    assert!(!out.contains("@wintermute"), "{out}");
}

// --- scrolling the preview, and keeping it steady -------------------------------------------------

fn wheel(state: &mut State, settings: &Settings, down: bool, column: u16, row: u16) {
    use crossterm::event::{Event, KeyModifiers, MouseEvent, MouseEventKind};
    let kind = if down {
        MouseEventKind::ScrollDown
    } else {
        MouseEventKind::ScrollUp
    };
    let event = Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
    let _ = state.handle_input(settings, &event);
}

/// The strip's lines, borders and scrollbar stripped.
fn strip(out: &str) -> String {
    section(out, "atuin · ai-resume", "╰").join("\n")
}

/// At the top the strip is the overview (each part a line or two); scrolled, it is the parts in
/// full, one after another, with a scrollbar, down to the end of the last reply and no further.
#[rstest]
#[tokio::test]
async fn the_strip_scrolls_through_the_parts_in_full() {
    use super::state::Pane;

    let s = settings();
    let mut state = loaded(&s, "", 0).await;
    let top = text(&render(&mut state, &s, 100, 30));
    assert!(strip(&top).contains("first  Build atuin ai resume"), "{top}");
    assert!(strip(&top).contains("last   Grouping done"), "{top}");
    assert!(top.contains('┃'), "a scrollbar, the text being longer: {top}");

    state.scroll_pane(Pane::Strip, 3);
    let out = text(&render(&mut state, &s, 100, 30));
    let scrolled = strip(&out);
    assert!(scrolled.contains("• resume on enter, edit on tab"), "{scrolled}");
    assert!(!scrolled.contains("first  Build"), "{scrolled}");
    // The metadata line stays put.
    assert!(out.contains("atuin · ai-resume · 1 fork"), "{out}");

    state.scroll_pane(Pane::Strip, 10_000);
    let out = text(&render(&mut state, &s, 100, 30));
    let end = strip(&out);
    assert!(end.contains("Tool calls and reasoning stay out of the preview."), "{end}");
    let scroll = &state.scrolls[Pane::Strip as usize];
    assert!(!scroll.more);
    assert_eq!(scroll.offset, scroll.len - scroll.height, "clamped to the end");

    // Another session starts at the top.
    state.list.selected = 1;
    let out = text(&render(&mut state, &s, 100, 30));
    assert!(out.contains("first  "), "{out}");
    state.list.selected = 0;
    let out = text(&render(&mut state, &s, 100, 30));
    assert_eq!(strip(&out), strip(&top));
}

/// The wheel scrolls the preview it is over, and moves the selection over the list. The mouse
/// reports screen rows, which an inline viewport's panes are drawn at.
#[rstest]
#[case::fullscreen(0)]
#[case::inline(12)]
#[tokio::test]
async fn the_wheel_scrolls_the_pane_under_it(#[case] origin: u16) {
    use ratatui::backend::Backend as _;
    use ratatui::{TerminalOptions, Viewport};

    use super::state::Pane;

    let s = settings();
    let mut state = loaded(&s, "", 0).await;
    let mut themes = ThemeManager::new(None, None);
    let theme = themes.load_theme("default", None);
    let mut backend = TestBackend::new(100, 42);
    backend.set_cursor_position((0, origin)).unwrap();
    let viewport = if origin == 0 {
        Viewport::Fullscreen
    } else {
        Viewport::Inline(30)
    };
    let mut terminal = Terminal::with_options(backend, TerminalOptions { viewport }).unwrap();
    let mut draw = |state: &mut State| {
        terminal.draw(|f| state.draw(f, &s, theme)).unwrap();
    };
    draw(&mut state);

    let area = state.scrolls[Pane::Strip as usize].area.unwrap();
    assert!(area.y >= origin, "{area:?}");
    wheel(&mut state, &s, true, area.x + 5, area.y + 1);
    assert_eq!(state.scrolls[Pane::Strip as usize].offset, super::state::WHEEL_LINES);
    assert_eq!(state.list.selected, 0, "the selection stays");
    draw(&mut state);
    wheel(&mut state, &s, false, area.x + 5, area.bottom() - 1);
    assert_eq!(state.scrolls[Pane::Strip as usize].offset, 0);

    // Over the list (a few rows above the strip): the selection moves up, as it looks.
    wheel(&mut state, &s, false, area.x + 5, area.y - 4);
    assert_eq!(state.list.selected, 1);
    wheel(&mut state, &s, true, area.x + 5, area.y - 4);
    assert_eq!(state.list.selected, 0);
    // The row above the viewport is the shell's, not the picker's.
    if origin > 0 {
        let above = state.scrolls[Pane::Strip as usize].area.unwrap().y - origin;
        assert!(above > 0);
    }
}

/// The keys scroll whichever preview is showing: the strip, the pane beside the list, or
/// Inspect's conversation (whose fields stay put).
#[rstest]
#[case::strip(0, 100, "atuin · ai-resume · 1 fork")]
#[case::side(0, 150, "Claude Code · claude-opus-4-5")]
#[case::inspect(1, 100, "Session   7f3c9a12")]
#[tokio::test]
async fn the_keys_scroll_the_preview_showing(
    #[case] tab: usize,
    #[case] width: u16,
    #[case] fixed: &str,
) {
    let s = settings();
    let mut state = loaded(&s, "", tab).await;
    let top = text(&render(&mut state, &s, width, 30));
    let _ = press(&mut state, &s, "shift-down");
    let _ = press(&mut state, &s, "alt-down");
    let _ = press(&mut state, &s, "shift-pagedown");
    let out = text(&render(&mut state, &s, width, 30));
    assert_ne!(out, top);
    assert!(out.contains(fixed), "{out}");
    assert!(!out.contains("First prompt") || tab == 0, "scrolled past its heading: {out}");
    assert_eq!(state.list.selected, 0, "the selection stays");
    let _ = press(&mut state, &s, "shift-pageup");
    let _ = press(&mut state, &s, "alt-up");
    let _ = press(&mut state, &s, "shift-up");
    let _ = press(&mut state, &s, "shift-up");
    assert_eq!(text(&render(&mut state, &s, width, 30)), top);
}

/// Moving to a session whose preview isn't read yet keeps showing the last one (not an empty
/// preview, which also shrank the strip and moved the list) until it is, or [`HOLD`] passes.
///
/// [`HOLD`]: super::state::HOLD
#[rstest]
#[tokio::test]
async fn the_preview_never_blanks_between_selections() {
    use std::time::{Duration, Instant};

    use super::state::HOLD;

    let s = settings();
    let mut state = loaded(&s, "", 0).await;
    let before = text(&render(&mut state, &s, 100, 30));
    state.preview_drawn(Instant::now());
    let input_row = |out: &str| out.lines().position(|l| l.contains("[  WORKSPACE")).unwrap();

    let next = state.results[1].clone();
    let preview = state.previews.remove(&next.handle).unwrap();
    state.list.selected = 1;
    let held = text(&render(&mut state, &s, 100, 30));
    state.preview_drawn(Instant::now());
    assert!(strip(&held).contains("first  Build atuin ai resume"), "{held}");
    assert!(!held.contains('…') || held.contains("search.…"), "no placeholder: {held}");
    assert_eq!(input_row(&held), input_row(&before), "the list doesn't move");
    assert!(state.hold_ends().is_some());

    // Past the hold, the session selected, waiting; the strip keeps its height.
    let later = Instant::now() + HOLD + Duration::from_millis(1);
    assert_eq!(state.preview_row_at(later).unwrap().handle, next.handle);

    state.apply_preview(next.handle.clone(), preview);
    assert_eq!(state.preview_row().unwrap().handle, next.handle);
    let after = text(&render(&mut state, &s, 100, 30));
    assert!(!strip(&after).contains("Build atuin ai resume"), "{after}");
    assert_eq!(input_row(&after), input_row(&before));
    assert_eq!(state.hold_ends(), None);
}

/// A live refresh reads the selected session's preview again: until the new one comes, the
/// frame is exactly what it was.
#[rstest]
#[tokio::test]
async fn a_refresh_keeps_the_frame_as_it_was() {
    for width in [100, 150] {
        let s = settings();
        let mut state = loaded(&s, "", 0).await;
        let before = text(&render(&mut state, &s, width, 30));
        let (generation, mode, filter) = state.refresh().unwrap();
        let selected = state.selected().unwrap().handle.clone();
        assert!(state.wants_preview(&selected), "the live session's preview is read again");
        assert_eq!(text(&render(&mut state, &s, width, 30)), before);
        let rows = FakeSource::new().search(&filter).await.unwrap();
        state.apply_results(generation, mode, rows);
        assert_eq!(text(&render(&mut state, &s, width, 30)), before);
    }
}

/// With `preview.strategy = "auto"`, the strip grows to fit a session's text but doesn't shrink
/// for a shorter one, so the list stays where it is as the selection moves.
#[rstest]
#[tokio::test]
async fn the_automatic_strip_height_only_grows() {
    let s = settings();
    let mut state = loaded(&s, "", 0).await;
    let input_row = |out: &str| out.lines().position(|l| l.contains("[  WORKSPACE")).unwrap();
    let tall = input_row(&text(&render(&mut state, &s, 100, 30)));
    let short = state
        .results
        .iter()
        .position(|r| r.title.text.starts_with("Prototype a live theme preview"))
        .unwrap();
    state.list.selected = short;
    assert_eq!(input_row(&text(&render(&mut state, &s, 100, 30))), tall);

    // Opened on the short one, it is short; the tall one grows it.
    let mut state = loaded(&s, "", 0).await;
    state.list.selected = short;
    let first = input_row(&text(&render(&mut state, &s, 100, 30)));
    state.list.selected = 0;
    assert!(input_row(&text(&render(&mut state, &s, 100, 30))) < first);
}

/// In a linked worktree, the workspace is the worktree's own checkout and the branch its own
/// `HEAD`, not the main checkout's.
#[cfg(unix)]
#[rstest]
fn a_worktree_is_its_own_workspace_and_branch() {
    use std::path::Path;

    use atuin_client::settings::AiSessionFilterMode as FilterMode;

    let git = |dir: &Path, args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(["-c", "user.name=atuin", "-c", "user.email=atuin@example.com"])
            .args(args)
            .current_dir(dir)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join("main");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q", "-b", "trunk"]);
    git(&main, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let worktree = tmp.path().join("feature-wt");
    git(&main, &["worktree", "add", "-q", "-b", "feature", worktree.to_str().unwrap()]);
    let deep = worktree.join("src");
    std::fs::create_dir_all(&deep).unwrap();

    assert_eq!(super::checkout(&main), (Some(main.clone()), Some("trunk".to_owned())));
    let (root, branch) = super::checkout(&deep);
    assert_eq!(root.as_deref(), Some(worktree.as_path()));
    assert_eq!(branch.as_deref(), Some("feature"));

    // The workspace and branch filters search the worktree's directory, on its branch.
    let s = settings();
    let mut ctx = fake::context();
    ctx.cwd = deep;
    ctx.git_root = root;
    ctx.branch = branch;
    let mut state = State::new(&s, ctx, "");
    state.mode = FilterMode::Branch;
    let filter = state.filter();
    assert_eq!(filter.db.workspace.as_deref(), Some(worktree.as_path()));
    assert_eq!(filter.db.branch.as_deref(), Some("feature"));

    // A detached worktree has no branch to filter by.
    git(&worktree, &["checkout", "-q", "--detach"]);
    assert_eq!(super::checkout(&worktree), (Some(worktree.clone()), None));
}

/// Enter on an expanded fork whose plan was asked for while browsing (and may since have given
/// way to another session's): the picker asks again, as an accept, and the answer finishes it,
/// though the root's plan is asked for after it.
#[rstest]
#[tokio::test]
async fn an_accept_always_gets_its_plan() {
    use std::sync::Arc;

    use super::state::{InputAction, PLAN, Pending};
    use super::{Outcome, accept, apply_response, complete, request_plan, worker};

    let s = settings();
    let (requests, mut responses) =
        worker::spawn(Arc::new(FakeSource::new()), Arc::new(FakeResumer::default()));
    let mut state = loaded(&s, "", 1).await;
    let root = state.selected().unwrap().clone();
    many_children(&mut state, 2);
    state.plans.clear();
    press(&mut state, &s, "c");
    let fork = state.target().unwrap().clone();
    assert_ne!(fork.handle, root.handle);
    // Asked for earlier, never answered.
    state.requested.insert((fork.handle.clone(), PLAN));

    assert_eq!(press(&mut state, &s, "enter"), InputAction::Resume);
    assert_eq!(accept(&mut state, Pending::Resume, &requests), None);
    assert_eq!(state.pending, Some((fork.handle.clone(), Pending::Resume)));
    // The selection settles on the root: its plan is asked for after the fork's.
    request_plan(&mut state, &requests, &root);

    let outcome = loop {
        let next = tokio::time::timeout(std::time::Duration::from_secs(10), responses.recv());
        let response = next.await.expect("the plan never came").unwrap();
        apply_response(&mut state, response, &requests);
        if let Some((handle, pending)) = state.pending.clone()
            && state.plans.contains_key(&handle)
        {
            break complete(&mut state, pending, &requests);
        }
    };
    let Some(Outcome::Resume(plan)) = outcome else {
        panic!("{outcome:?}");
    };
    assert!(plan.args.join(" ").ends_with(fork.handle.session.as_ref()), "{plan:?}");
}

/// While the daemon rebuilds the index, the status row says so (and how far it has got), the
/// picker otherwise as usable as ever; a message of its own takes the row meanwhile.
#[rstest]
#[tokio::test]
async fn a_rebuild_in_progress_is_said_in_the_status_row() {
    use atuin_client::theme::Meaning;

    use super::rebuild::Rebuilding;

    let s = settings();
    let mut state = loaded(&s, "", 0).await;
    let before = text(&render(&mut state, &s, 100, 30));
    assert!(!before.contains("rebuilding"), "{before}");

    state.rebuilding = Some(Rebuilding {
        progress: Some((1200, 5000)),
    });
    let out = text(&render(&mut state, &s, 100, 30));
    let line = "rebuilding the session index: 1200 of 5000 records; results may be incomplete";
    assert!(out.lines().last().unwrap().contains(line), "{out}");
    assert!(out.contains("Add an interactive resume picker"), "the list still shows: {out}");
    let selected = state.list.selected;
    press(&mut state, &s, "up");
    assert_ne!(state.list.selected, selected, "and still moves");

    state.status = Some(("copied: claude --resume x".to_owned(), Meaning::AlertInfo));
    let out = text(&render(&mut state, &s, 100, 30));
    assert!(out.contains("copied: claude --resume x") && !out.contains(line), "{out}");
}

/// However small the terminal, every view draws without panicking: the list and Inspect (with
/// its forks collapsed and expanded), in each style, inverted or not, and with or without a
/// query, at widths 0-4 and heights 0-3, and a little beyond.
#[rstest]
#[tokio::test]
async fn tiny_terminals_draw_without_panicking(
    #[values(UiStyle::Auto, UiStyle::Full, UiStyle::Compact)] style: UiStyle,
    #[values(false, true)] invert: bool,
) {
    let mut s = settings();
    s.style = style;
    s.invert = invert;
    let widths: Vec<u16> = (0..=12).chain([40, 100, 200]).collect();
    let heights: Vec<u16> = (0..=8).chain([30]).collect();
    let views = [("", 0, false), ("flaky", 0, false), ("", 1, false), ("", 1, true)];
    for (query, tab, expanded) in views {
        let mut state = loaded(&s, query, tab).await;
        if expanded {
            many_children(&mut state, 5);
            press(&mut state, &s, "c");
        }
        for &width in &widths {
            for &height in &heights {
                render(&mut state, &s, width, height);
            }
        }
    }
}
