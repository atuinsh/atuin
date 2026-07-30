//! Minimal demo that renders the interactive search UI to the terminal.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p atuin-ui --example demo
//! ```
//!
//! Scroll with the arrow keys / `j`/`k` / PageUp/PageDown / Home/End.
//! Press `q`, `Esc`, or `Ctrl-C` to exit.

use std::io;
use std::ops::Range;
use std::time::{SystemTime, UNIX_EPOCH};

use atuin_ui::models::{HistoryList, HistoryRow, HistorySource, Mode, Model};
use atuin_ui::runtime;
use atuin_ui::search::interactive::{SearchInteractive, build_turtle_logo};
use atuin_ui::theme::Theme;
use ratatui::style::{Color, Style};
use tracing_chrome::ChromeLayerBuilder;
use tracing_subscriber::prelude::*;

/// A fake history source: several million synthetic rows, generated on demand —
/// proof that the list virtualizes (the resident window stays tiny).
#[derive(Clone)]
struct SyntheticHistory {
    total: usize,
    /// Unix seconds "now" — the newest row ran at this time.
    base_time: i64,
}

impl SyntheticHistory {
    fn row(&self, i: usize) -> HistoryRow {
        let n = i as i64;
        let command = match i % 6 {
            0 => format!("git commit -m \"fix bug #{i}\""),
            1 => "ls -la | grep foo".to_string(),
            2 => "cargo build --release && ./run.sh".to_string(),
            3 => format!("echo $HOME/{i} > out.txt"),
            4 => "docker run -it --rm ubuntu bash # debug".to_string(),
            _ => "find . -name '*.rs' | xargs wc -l".to_string(),
        };
        HistoryRow {
            id: format!("row-{i}"),
            command,
            timestamp: self.base_time - n * 90,
            duration: ((n * 37) % 3000 + 5) * 1_000_000,
            exit: if i.is_multiple_of(9) { 1 } else { 0 },
        }
    }
}

impl HistorySource for SyntheticHistory {
    async fn total(&self) -> usize {
        self.total
    }

    async fn load(&self, range: Range<usize>) -> Vec<HistoryRow> {
        range.map(|i| self.row(i)).collect()
    }

    async fn search(&self, query: &str) -> Vec<HistoryRow> {
        let needle = query.to_lowercase();
        // Scan a bounded prefix of the synthetic history; collect up to 200 hits.
        (0..self.total.min(50_000))
            .map(|i| self.row(i))
            .filter(|r| r.command.to_lowercase().contains(&needle))
            .take(200)
            .collect()
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    // With ATUIN_UI_TRACE=1, write a Chrome trace of the render spans to
    // ./trace-<ts>.json (open in https://ui.perfetto.dev or chrome://tracing).
    // It writes to a file, so it never disturbs the TUI. The guard flushes the
    // trace when the demo exits, so hold it until `main` returns.
    let _trace_guard = std::env::var_os("ATUIN_UI_TRACE").map(|_| {
        let (layer, guard) = ChromeLayerBuilder::new().build();
        tracing_subscriber::registry()
            .with(layer.with_filter(tracing_subscriber::filter::LevelFilter::TRACE))
            .init();
        guard
    });

    // Detect the terminal's graphics protocol BEFORE the runtime enters the
    // alternate screen — `from_query_stdio` needs a normal TTY.
    let logo = build_turtle_logo();

    // Override the banner styles; inherit the syntax/time/duration colours.
    let theme = Theme {
        base: Style::default().fg(Color::White).bg(Color::DarkGray),
        error: Style::default().fg(Color::White).bg(Color::Red),
        annotation: Style::default().fg(Color::Gray),
        ..Default::default()
    };
    // `ATUIN_UI_VIM=1` boots the vim-style modal interface (in SEARCH mode).
    let mode = std::env::var_os("ATUIN_UI_VIM").map(|_| Mode::Search);
    let model = Model {
        theme,
        enter_accept: true,
        history: HistoryList::new(),
        search: atuin_ui::models::SearchInput::new(),
        mode,
    };

    let base_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let source = SyntheticHistory {
        total: 5_000_000,
        base_time,
    };

    // Inline (scrollback): a titlebar row plus a small history viewport.
    runtime::run_inline(SearchInteractive::new(model, logo, source), 12).await
}
