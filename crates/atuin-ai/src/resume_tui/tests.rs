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
    assert!(out.contains("│       atuin · main · @buildbox"), "{out}");
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
#[case::wide(100)]
#[case::narrow(64)]
#[tokio::test]
async fn preview_renders_markdown(#[case] width: u16) {
    let s = markdown_settings(15);
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
    // Where it ran takes the first line, leaving three for the text.
    let s = markdown_settings(4);
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
        ("full, 80x24", &s, "", 0, 80, 24),
        ("full, split, 120x34", &s, "", 0, 120, 34),
        ("full, inspect (ctrl-o), 80x24", &s, "", 1, 80, 24),
    ] {
        frames.push(format!("=== {label} ===\n{}", frame(settings, query, tab, w, h).await));
    }
    let mut global = s.clone();
    global.ai.sessions.filter_mode = Some(atuin_client::settings::AiSessionFilterMode::Global);
    for (w, h) in [(100, 30), (80, 24), (120, 34), (160, 40)] {
        frames.push(format!(
            "=== global 'h:claude', {w}x{h} ===\n{}",
            frame(&global, "h:claude", 0, w, h).await
        ));
    }
    for (w, h) in [(60, 24), (80, 24), (120, 34)] {
        frames.push(format!("=== global, {w}x{h} ===\n{}", frame(&global, "", 0, w, h).await));
    }
    // Another host's session selected: only the preview says where it ran.
    for (w, h) in [(80, 24), (120, 34)] {
        let out = frame(&global, "aarch64", 0, w, h).await;
        frames.push(format!("=== global 'aarch64' (another host's), {w}x{h} ===\n{out}"));
    }

    // Inspect with a long list of forks, collapsed and expanded (`c`, then down twice).
    for (w, h) in [(80, 24), (120, 34)] {
        let mut state = loaded(&s, "", 1).await;
        many_children(&mut state, 17);
        let collapsed = text(&render(&mut state, &s, w, h));
        frames.push(format!("=== inspect, 17 forks, {w}x{h} ===\n{collapsed}"));
        for key in ["c", "down", "down"] {
            press(&mut state, &s, key);
        }
        let expanded = text(&render(&mut state, &s, w, h));
        frames.push(format!("=== inspect, 17 forks expanded, {w}x{h} ===\n{expanded}"));
    }

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
    state.flattened.insert((row.handle, None), Ok(flattened));
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
    assert_eq!(status, "Pi can't resume it here: pick another line", "the line says why");
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

/// The worker's answer to the catch-up the selected session is waiting on, as the fake source
/// and `resumer` give it.
/// What the worker answers a request for the selected session's heads with, from `source`.
async fn answer_heads(state: &mut State, source: &FakeSource, requests: &super::worker::Requests) {
    let row = state.selected().unwrap().clone();
    let heads = source.heads(&row.handle).await.unwrap();
    assert!(state.requested.contains(&(row.handle.clone(), super::state::HEADS)), "asked");
    super::apply_response(state, super::worker::Response::Heads(row.handle, heads), requests);
}

async fn answer_sync(state: &mut State, resumer: &dyn Resumer) {
    let row = state.selected().unwrap().clone();
    let head = state.picked.get(&row.handle).cloned();
    let synced = resumer.sync(&FakeSource::new(), &row, head.as_ref()).await;
    state.requested.remove(&(row.handle.clone(), super::state::SYNC));
    state.synced.insert(row.handle, (head, synced));
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

    // Without the chooser, it catches the session up with sync (the worker), then resumes.
    for action in [Pending::Resume, Pending::Edit] {
        let mut state = loaded(&s, "", 0).await;
        let outcome = accept(&mut state, action, resumer.as_ref(), &requests, false);
        assert_eq!(outcome, None);
        assert!(state.chooser.is_none());
        assert_eq!(state.pending.as_ref().map(|(_, a)| *a), Some(action));
        answer_sync(&mut state, resumer.as_ref()).await;
        let outcome = accept(&mut state, action, resumer.as_ref(), &requests, false);
        match action {
            Pending::Resume => assert!(matches!(outcome, Some(Outcome::Resume(_))), "{outcome:?}"),
            _ => assert!(matches!(outcome, Some(Outcome::Edit(_))), "{outcome:?}"),
        }
    }

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
    // The dimmed line says why; the status line doesn't say it again.
    assert_eq!(state.status, None);
    let out = text(&render(&mut state, &s, 100, 30));
    assert_eq!(out.matches("the session's directory is gone").count(), 1, "{out}");
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
        let continued = resumer.continue_in(&source, &row, HarnessKind::Codex, None).await;
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
    let lines = super::panel::tree_lines(
        &root.handle,
        &[child],
        fake::now(),
        time::UtcOffset::UTC,
        90,
        theme,
    );
    let line: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(line.contains("continued in Codex · fix the flaky test"), "{line}");
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
            assert!(!remote.contains("@buildbox"), "{width}: {remote}");
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

/// A session that is still running says what resuming it does, without stopping it.
#[rstest]
#[tokio::test]
async fn the_chooser_says_a_live_session_is_running() {
    use super::state::{InputAction, Pending};

    let s = settings();
    let mut state = with_chooser(&s, "", Pending::Resume).await;
    let out = text(&render(&mut state, &s, 100, 30));
    assert!(
        out.contains("> 1 CC Claude Code  original · running elsewhere — resuming forks it"),
        "{out}"
    );
    assert_eq!(press(&mut state, &s, "enter"), InputAction::Pick(None, Pending::Resume));

    // Not once it has stopped.
    let mut state = with_chooser(&s, "flaky", Pending::Resume).await;
    let out = text(&render(&mut state, &s, 100, 30));
    assert!(out.contains("> 1 CX Codex        original "), "{out}");
    assert!(!out.contains("running elsewhere"), "{out}");
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

// --- branches: a session that went on on several machines ---------------------------------------

/// The diverged fake session (`fake::DIVERGED`) selected, in the global filter.
async fn diverged(settings: &Settings, tab: usize) -> State {
    let mut state = loaded(settings, "dotfiles sync redesign", tab).await;
    state.apply_host_names(FakeSource::new().host_names().await.unwrap());
    assert_eq!(state.selected().unwrap().handle.session.as_ref(), fake::DIVERGED);
    state
}

/// A diverged session asks which branch: one line of its own harness per branch, newest first
/// and selected, then the other harnesses. Picking a branch line resumes that branch.
#[rstest]
#[tokio::test]
async fn the_chooser_lists_the_branches_of_a_diverged_session() {
    use super::state::{InputAction, Pending};

    let s = chooser_settings();
    let mut state = diverged(&s, 0).await;
    let row = state.selected().unwrap().clone();
    state.open_chooser(FakeResumer::default().continue_targets(&row), Pending::Resume);
    let out = text(&render(&mut state, &s, 100, 30));
    let dump = out.lines().filter(|l| l.contains('│') || l.contains('╭') || l.contains('╰'));
    println!("{}", dump.collect::<Vec<_>>().join("\n"));
    assert!(out.contains("2 branches: it went on separately on several machines"), "{out}");
    assert!(out.contains("> 1 CC Claude Code  @buildbox · 5h · 40 msgs (latest)"), "{out}");
    assert!(out.contains("  2 CC Claude Code  this machine · yest · 24 msgs"), "{out}");
    assert!(out.contains("  3 CX Codex        continue"), "{out}");

    // Enter resumes the latest; a digit, the other.
    assert_eq!(press(&mut state, &s, "enter"), InputAction::Pick(None, Pending::Resume));
    assert_eq!(state.picked[&row.handle].as_ref(), "b40");
    state.open_chooser(Vec::new(), Pending::Resume);
    assert_eq!(state.chooser.as_ref().unwrap().selected, 0, "reopens on the branch picked");
    assert_eq!(press(&mut state, &s, "2"), InputAction::Pick(None, Pending::Resume));
    assert_eq!(state.picked[&row.handle].as_ref(), "h24");
    state.open_chooser(Vec::new(), Pending::Resume);
    assert_eq!(state.chooser.as_ref().unwrap().selected, 1);
    // Without other harnesses, only the branches.
    assert_eq!(state.chooser.as_ref().unwrap().len(), 2);
}

/// The chooser's box, as drawn.
fn chooser_box(out: &str) -> String {
    let lines: Vec<&str> = out.lines().collect();
    let top = lines.iter().position(|l| l.contains("╭ Resume in ")).unwrap();
    let bottom = top + lines[top..].iter().position(|l| l.contains('╰')).unwrap();
    let top_line: Vec<char> = lines[top].chars().collect();
    let start = top_line.iter().position(|c| *c == '╭').unwrap();
    let end = top_line.iter().position(|c| *c == '╮').unwrap();
    lines[top..=bottom]
        .iter()
        .map(|l| l.chars().skip(start).take(end - start + 1).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// For a diverged session, the other harnesses' lines continue the branch the branch lines
/// picked, under a heading naming it, each saying what continuing that branch flattens: moving
/// onto a branch line switches them to it, moving on down to them keeps it (marked `•`), and
/// picking one continues that branch.
#[rstest]
#[tokio::test]
async fn the_chooser_continues_the_branch_picked() {
    use atuin_client::ai_session::{HarnessKind, SourceId};
    use atuin_common::harnesstools::continuation::Flattened;

    use super::state::{InputAction, Pending};

    let s = chooser_settings();
    let mut state = diverged(&s, 0).await;
    let row = state.selected().unwrap().clone();
    let targets = vec![HarnessKind::Codex, HarnessKind::Pi];
    state.open_chooser(targets, Pending::Resume);
    let flattened = |calls| {
        Ok(Flattened {
            tool_calls: calls,
            tool_results: calls,
            reasoning: 1,
        })
    };
    let head = |id: &str| Some(SourceId::from(id.to_owned()));
    state.flattened.insert((row.handle.clone(), head("b40")), flattened(12));
    state.flattened.insert((row.handle.clone(), head("h24")), flattened(3));

    let mut frames = Vec::new();
    let mut frame = |state: &mut State| {
        let out = chooser_box(&text(&render(state, &s, 100, 30)));
        frames.push(out.clone());
        out
    };
    let out = frame(&mut state);
    assert!(out.contains("> 1 CC Claude Code  @buildbox · 5h · 40 msgs (latest)"), "{out}");
    assert!(out.contains("Continue @buildbox's branch in:"), "{out}");
    assert!(out.contains("  3 CX Codex        12 tool calls become notes, reasoning dropped"));

    press(&mut state, &s, "down");
    let out = frame(&mut state);
    assert!(out.contains("> 2 CC Claude Code  this machine · yest · 24 msgs"), "{out}");
    assert!(out.contains("Continue this machine's branch in:"), "{out}");
    assert!(out.contains("  3 CX Codex        3 tool calls become notes, reasoning dropped"));

    press(&mut state, &s, "down");
    let out = frame(&mut state);
    assert!(out.contains("• 2 CC Claude Code  this machine"), "{out}");
    assert!(out.contains("> 3 CX Codex        3 tool calls become notes"), "{out}");
    println!("{}", frames.join("\n\n"));

    assert_eq!(
        press(&mut state, &s, "enter"),
        InputAction::Pick(Some(HarnessKind::Codex), Pending::Resume)
    );
    assert_eq!(state.continue_from[&row.handle].as_ref(), "h24");
    assert!(!state.picked.contains_key(&row.handle), "no branch resumed in its own harness");

    // Straight to a harness line from the latest branch: it continues the latest.
    state.open_chooser(vec![HarnessKind::Codex, HarnessKind::Pi], Pending::Resume);
    assert_eq!(
        press(&mut state, &s, "4"),
        InputAction::Pick(Some(HarnessKind::Pi), Pending::Resume)
    );
    assert_eq!(state.continue_from[&row.handle].as_ref(), "b40");
}

/// ctrl-y on a branch line copies a command resuming that branch, named by the start of its id
/// (`--branch`); on a harness line, one continuing the branch its lines are for. A session that
/// went one way copies its harness's own command, as before.
#[rstest]
#[tokio::test]
async fn copying_a_branch_names_it() {
    use atuin_client::ai_session::HarnessKind;

    use super::state::{InputAction, Pending};
    use super::{branch_arg, resume_line};

    let s = chooser_settings();
    let mut state = diverged(&s, 0).await;
    let row = state.selected().unwrap().clone();
    let resume = FakeResumer::default().plan(&row).await.unwrap();
    let targets = vec![HarnessKind::Codex, HarnessKind::Pi];

    state.open_chooser(targets.clone(), Pending::Resume);
    press(&mut state, &s, "down");
    assert_eq!(press(&mut state, &s, "ctrl-y"), InputAction::Pick(None, Pending::Copy));
    assert!(state.chooser.is_some(), "copying leaves the chooser open");
    let id = fake::DIVERGED;
    assert_eq!(resume_line(&state, &row, &resume), format!("atuin ai resume {id} --branch h24"));

    press(&mut state, &s, "up");
    press(&mut state, &s, "ctrl-y");
    assert_eq!(resume_line(&state, &row, &resume), format!("atuin ai resume {id} --branch b40"));

    // A harness line continues the branch its lines are for.
    state.open_chooser(targets, Pending::Resume);
    press(&mut state, &s, "down");
    press(&mut state, &s, "down");
    assert_eq!(
        press(&mut state, &s, "ctrl-y"),
        InputAction::Pick(Some(HarnessKind::Codex), Pending::Copy)
    );
    assert_eq!(branch_arg(&row, state.continue_from.get(&row.handle)), " --branch h24");

    // One way: no branch to name, the harness's own command.
    let one_way = loaded(&s, "", 0).await;
    let row = one_way.selected().unwrap().clone();
    assert!(row.branches().is_empty());
    let resume = FakeResumer::default().plan(&row).await.unwrap();
    assert_eq!(resume_line(&one_way, &row, &resume), super::resumer::shell_line(&resume.plan));
}

/// Continuing a diverged session in another harness writes out the branch picked, else the
/// newest.
#[rstest]
#[case::picked(Some("h24"), "h", 24)]
#[case::the_newest(None, "b", 40)]
#[tokio::test]
async fn continuing_a_diverged_session_writes_the_branch_picked(
    #[case] picked: Option<&str>,
    #[case] prefix: &str,
    #[case] rows: usize,
) {
    use atuin_client::ai_session::{HarnessKind, SourceId};

    let source = FakeSource::new();
    let row = source.find_by_id(fake::DIVERGED).await.unwrap().remove(0);
    let tmp = tempfile::tempdir().unwrap();
    let context = super::ResumeContext {
        cwd: tmp.path().to_owned(),
        ..fake::context()
    };
    let written = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    let resumer = super::resumer::HarnessResumer::on(
        context,
        atuin_client::settings::AiSessionResume::default(),
        Recording(written.clone()),
    );
    let head = picked.map(|p| SourceId::from(p.to_owned()));
    resumer.continue_in(&source, &row, HarnessKind::Pi, head.as_ref()).await.unwrap();
    let written = written.lock().clone();
    // Under new ids, the branch's messages.
    let messages = &written[0].messages;
    assert_eq!(messages.len(), 6 + rows);
    let last = &messages.last().unwrap().content;
    let expected = format!("{prefix} {rows}");
    assert!(
        matches!(last.as_slice(), [atuin_common::harnesstools::session::Content::Text(t)] if *t == expected),
        "{last:?}"
    );
}

/// A machine where every program is installed, and rehydrating records what it would write.
struct Recording(
    std::sync::Arc<
        parking_lot::Mutex<Vec<atuin_common::harnesstools::rehydrate::RehydrateSession>>,
    >,
);

#[async_trait::async_trait]
impl super::resumer::Machine for Recording {
    async fn locate(
        &self,
        _: atuin_common::harnesstools::AnyHarness,
        _: &str,
    ) -> Option<std::path::PathBuf> {
        None
    }

    async fn rehydrate(
        &self,
        _: atuin_common::harnesstools::AnyHarness,
        session: &atuin_common::harnesstools::rehydrate::RehydrateSession,
    ) -> Result<std::path::PathBuf, atuin_common::harnesstools::rehydrate::RehydrateError> {
        self.0.lock().push(session.clone());
        Ok(std::path::PathBuf::from("/written.jsonl"))
    }

    fn installed(&self, _: &str) -> bool {
        true
    }

    async fn local_tip(
        &self,
        _: atuin_common::harnesstools::AnyHarness,
        _: &str,
    ) -> Result<
        Option<atuin_common::harnesstools::sync::LocalTip>,
        atuin_common::harnesstools::sync::SyncError,
    > {
        Ok(None)
    }

    async fn is_live(
        &self,
        _: atuin_common::harnesstools::AnyHarness,
        _: &str,
        _: Option<&std::path::Path>,
    ) -> atuin_common::harnesstools::sync::Liveness {
        atuin_common::harnesstools::sync::Liveness::NotLive
    }

    async fn append(
        &self,
        _: atuin_common::harnesstools::AnyHarness,
        _: &str,
        _: &atuin_common::harnesstools::sync::LocalTip,
        _: &[atuin_common::harnesstools::rehydrate::RehydrateMessage],
        _: &atuin_common::harnesstools::sync::AppendOptions<'_>,
    ) -> Result<
        atuin_common::harnesstools::sync::AppendOutcome,
        atuin_common::harnesstools::sync::SyncError,
    > {
        Err(atuin_common::harnesstools::sync::SyncError::NotFound)
    }
}

/// Accepting a diverged session opens the chooser even with `resume_chooser = false`: a branch
/// has to be picked. The catch-up then asks for the branch picked.
#[rstest]
#[tokio::test]
async fn a_diverged_session_always_asks_which_branch() {
    use std::sync::Arc;

    use super::state::{InputAction, Pending};
    use super::{Outcome, accept, resume_original, worker};

    let s = chooser_settings();
    let resumer = Arc::new(FakeResumer::default());
    let (requests, _responses) = worker::spawn(Arc::new(FakeSource::new()), resumer.clone());
    let mut state = diverged(&s, 0).await;
    assert_eq!(accept(&mut state, Pending::Resume, resumer.as_ref(), &requests, false), None);
    let chooser = state.chooser.as_ref().expect("asks");
    assert!(chooser.targets.is_empty());
    assert_eq!(press(&mut state, &s, "2"), InputAction::Pick(None, Pending::Resume));
    assert_eq!(resume_original(&mut state, Pending::Resume, resumer.as_ref(), &requests), None);
    answer_heads(&mut state, &FakeSource::new(), &requests).await;
    assert_eq!(resume_original(&mut state, Pending::Resume, resumer.as_ref(), &requests), None);
    assert_eq!(state.status.as_ref().unwrap().0, "catching up with sync…");
    answer_sync(&mut state, resumer.as_ref()).await;
    assert_eq!(state.synced.values().next().unwrap().0.as_ref().map(AsRef::as_ref), Some("h24"));
    let outcome = resume_original(&mut state, Pending::Resume, resumer.as_ref(), &requests);
    assert!(matches!(outcome, Some(Outcome::Resume(_))), "{outcome:?}");
}

/// The preview, the detail pane and Inspect say a session went on in several branches; the row
/// doesn't.
#[rstest]
#[tokio::test]
async fn a_diverged_session_shows_its_branches() {
    let s = chooser_settings();
    let mut state = diverged(&s, 0).await;
    let out = text(&render(&mut state, &s, 100, 30));
    let row = out.lines().find(|l| l.contains("Plan the dotfiles sync")).unwrap();
    assert!(!row.contains("branches"), "{row}");
    assert!(out.contains("dotfiles · main · 2 branches"), "{out}");
    // Every branch's messages, the six they share counted once: 6 + 40 + 24.
    assert!(row.trim_end_matches(['│', ' ']).ends_with(" 70"), "{row}");

    let out = text(&render(&mut state, &s, 150, 40));
    assert!(out.contains("70 messages · 2 branches · started"), "{out}");

    let mut state = diverged(&s, 1).await;
    let out = text(&render(&mut state, &s, 100, 40));
    println!("{out}");
    assert!(out.contains(" Messages  70"), "{out}");
    assert!(out.contains(" Branches  @buildbox · 5h · 40 msgs  (latest)"), "{out}");
    assert!(out.contains("           this machine · yest · 24 msgs"), "{out}");
}

/// A branch another host wrote to minutes ago asks first, in a popup: resuming here will branch
/// the session. Enter goes on (and isn't asked again), esc goes back. Whether it did is read
/// again when the session is accepted: the list's heads are from the last search, which may be
/// minutes old.
#[rstest]
#[tokio::test]
async fn resuming_a_branch_live_elsewhere_warns_first() {
    use std::sync::Arc;

    use super::state::{InputAction, Pending};
    use super::{Outcome, resume_original, worker};

    let s = chooser_settings();
    let resumer = Arc::new(FakeResumer::default());
    let (requests, _responses) = worker::spawn(Arc::new(FakeSource::new()), resumer.clone());
    let mut state = diverged(&s, 0).await;
    let handle = state.selected().unwrap().handle.clone();
    // Buildbox went on since the list was searched.
    let mut branches = fake::diverged_branches();
    branches.heads.heads[0].last_at = fake::now() - time::Duration::minutes(2);
    let source = FakeSource::new().with_branches(&handle, branches);
    assert!(
        state.live_elsewhere(state.selected().unwrap(), Pending::Resume).is_none(),
        "not by the list's heads"
    );

    assert_eq!(resume_original(&mut state, Pending::Resume, resumer.as_ref(), &requests), None);
    assert!(state.warning.is_none(), "not before the heads are read again");
    answer_heads(&mut state, &source, &requests).await;
    assert_eq!(resume_original(&mut state, Pending::Resume, resumer.as_ref(), &requests), None);
    let warning = state.warning.clone().expect("warns");
    assert_eq!(warning.host, "@buildbox");
    assert!(state.pending.is_none(), "nothing asked of the worker yet");
    let out = text(&render(&mut state, &s, 100, 30));
    let dump = out.lines().filter(|l| l.contains('│') || l.contains('╭') || l.contains('╰'));
    println!("{}", dump.collect::<Vec<_>>().join("\n"));
    assert!(out.contains("╭ Still active elsewhere "), "{out}");
    assert!(out.contains("Still active on @buildbox (2m ago)."), "{out}");
    assert!(out.contains("Resuming here will branch the session."), "{out}");
    assert!(out.contains("<enter>: continue  <esc>: back"), "{out}");

    // Keys go to it, not the list; esc goes back.
    assert_eq!(press(&mut state, &s, "x"), InputAction::Continue);
    assert_eq!(press(&mut state, &s, "esc"), InputAction::Continue);
    assert!(state.warning.is_none());
    assert_eq!(state.input.as_str(), "dotfiles sync redesign");

    // Asked again, from heads read again.
    assert_eq!(resume_original(&mut state, Pending::Resume, resumer.as_ref(), &requests), None);
    answer_heads(&mut state, &source, &requests).await;
    assert_eq!(resume_original(&mut state, Pending::Resume, resumer.as_ref(), &requests), None);
    assert!(state.warning.is_some());
    assert_eq!(press(&mut state, &s, "enter"), InputAction::Pick(None, Pending::Resume));
    assert!(state.warning.is_none() && state.accept);
    assert_eq!(resume_original(&mut state, Pending::Resume, resumer.as_ref(), &requests), None);
    assert!(state.warning.is_none(), "not asked again");
    answer_sync(&mut state, resumer.as_ref()).await;
    let outcome = resume_original(&mut state, Pending::Resume, resumer.as_ref(), &requests);
    assert!(matches!(outcome, Some(Outcome::Resume(_))), "{outcome:?}");
}

/// A catch-up that couldn't write (the harness has it open here) keeps the picker open saying
/// why; enter then resumes the copy here as it is. One that did leaves its status as the note.
#[rstest]
#[tokio::test]
async fn a_held_catch_up_stays_open_and_then_resumes_as_it_is() {
    use std::sync::Arc;

    use atuin_client::theme::Meaning;

    use super::catchup::{Caught, Kept, Synced};
    use super::state::Pending;
    use super::{Outcome, finish_sync, resume_original, worker};

    let s = chooser_settings();
    let resumer = Arc::new(FakeResumer::default());
    let (requests, _responses) = worker::spawn(Arc::new(FakeSource::new()), resumer.clone());
    let mut state = loaded(&s, "", 0).await;
    let row = state.selected().unwrap().clone();
    let plan = resumer.plan(&row).await.unwrap().plan;

    let held = Synced {
        caught: Caught::Kept(Kept::Live { pid: Some(7) }),
        ..Synced::up_to_date(plan.clone())
    };
    assert_eq!(finish_sync(&mut state, &row, Ok(held), Pending::Resume), None);
    let (status, meaning) = state.status.clone().unwrap();
    assert_eq!(
        status,
        "Claude Code is running this session here — close it to catch up; enter resumes this \
         machine's copy as it is"
    );
    assert_eq!(meaning, Meaning::AlertWarn);
    let outcome = resume_original(&mut state, Pending::Resume, resumer.as_ref(), &requests);
    assert_eq!(outcome, Some(Outcome::Resume(plan.clone())));

    let mut state = loaded(&s, "", 0).await;
    let caught = Synced {
        caught: Caught::FastForwarded { messages: 136 },
        head: Some(fake::diverged_branches().heads.heads[0].clone()),
        ..Synced::up_to_date(plan.clone())
    };
    let outcome = finish_sync(&mut state, &row, Ok(caught), Pending::Edit);
    assert_eq!(outcome, Some(Outcome::Edit(plan)));
    assert_eq!(state.note.as_deref(), Some("caught up 136 messages from @buildbox"));
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
