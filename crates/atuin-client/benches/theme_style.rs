use atuin_client::theme::{Meaning, Theme, ThemeManager};
use crossterm::style::ContentStyle;

// The interactive search redraws every visible row on each keystroke and each
// scroll, and the command column is styled one character at a time. The old
// path called `Theme::as_style` for every character, and each call probes a
// `HashMap` (a `contains_key` plus an index). The new path resolves the handful
// of syntax meanings once per row into a small table and then matches per
// character.
//
// These two benches run over the *identical* per-row classification vectors so
// the relative comparison is robust to load from other parallel builds.

/// A realistic screenful of history: enough visible rows and command lengths to
/// total roughly 2400 character-style resolutions per redraw.
const COMMANDS: &[&str] = &[
    "git commit -m 'initial commit' --amend",
    "cargo build --release --workspace",
    "docker run -it --rm -v $HOME/app:/srv ubuntu bash",
    "grep -rn 'TODO' src/ | wc -l",
    "kubectl get pods -n prod -o wide",
    "ssh user@host 'tail -f /var/log/syslog'",
    "echo $PATH && export FOO=bar # set the var",
    "find . -name '*.rs' -type f | xargs wc -l",
    "psql -h db.internal -U admin -c 'select 1'",
    "curl -fsSL https://example.com/install.sh | sh",
];

const ROWS: usize = 48;

/// Approximate the shape of `syntax::classify`'s output for a shell command:
/// the first token is the command, `-`/`--` tokens are flags, quoted spans are
/// strings, `$`-prefixed tokens are variables, `#` begins a comment, shell
/// operators are operators, and everything else stays `Base`. The realism that
/// matters is the per-byte *distribution* of meanings; both benched paths see
/// the same vector.
fn classify(cmd: &str) -> Vec<Meaning> {
    let bytes = cmd.as_bytes();
    let mut meanings = vec![Meaning::Base; bytes.len()];
    let mut i = 0;
    let mut first_token = true;
    while i < bytes.len() {
        match bytes[i] {
            b' ' => i += 1,
            b'\'' | b'"' => {
                let quote = bytes[i];
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                i = (i + 1).min(bytes.len()); // include the closing quote
                meanings[start..i].fill(Meaning::SyntaxString);
                first_token = false;
            }
            b'#' => {
                meanings[i..].fill(Meaning::SyntaxComment);
                break;
            }
            b'|' | b'&' | b';' | b'>' | b'<' => {
                let start = i;
                while i < bytes.len() && matches!(bytes[i], b'|' | b'&' | b';' | b'>' | b'<') {
                    i += 1;
                }
                meanings[start..i].fill(Meaning::SyntaxOperator);
                first_token = true; // the next token begins a fresh command
            }
            _ => {
                let start = i;
                while i < bytes.len() && !matches!(bytes[i], b' ' | b'\'' | b'"') {
                    i += 1;
                }
                let meaning = match bytes[start] {
                    b'-' => Meaning::SyntaxFlag,
                    b'$' => Meaning::SyntaxVariable,
                    _ if first_token => Meaning::SyntaxCommand,
                    _ => Meaning::Base,
                };
                meanings[start..i].fill(meaning);
                first_token = false;
            }
        }
    }
    meanings
}

fn frame() -> Vec<Vec<Meaning>> {
    COMMANDS.iter().cycle().take(ROWS).map(|cmd| classify(cmd)).collect()
}

/// A replica of the per-row style table added to the real render loop in
/// `crates/atuin/src/command/client/search/history_list.rs`.
struct SyntaxStyles {
    base: ContentStyle,
    command: ContentStyle,
    flag: ContentStyle,
    string: ContentStyle,
    variable: ContentStyle,
    operator: ContentStyle,
    comment: ContentStyle,
}

impl SyntaxStyles {
    fn resolve(theme: &Theme) -> Self {
        Self {
            base: theme.as_style(Meaning::Base),
            command: theme.as_style(Meaning::SyntaxCommand),
            flag: theme.as_style(Meaning::SyntaxFlag),
            string: theme.as_style(Meaning::SyntaxString),
            variable: theme.as_style(Meaning::SyntaxVariable),
            operator: theme.as_style(Meaning::SyntaxOperator),
            comment: theme.as_style(Meaning::SyntaxComment),
        }
    }

    fn get(&self, meaning: Meaning) -> ContentStyle {
        match meaning {
            Meaning::SyntaxCommand => self.command,
            Meaning::SyntaxFlag => self.flag,
            Meaning::SyntaxString => self.string,
            Meaning::SyntaxVariable => self.variable,
            Meaning::SyntaxOperator => self.operator,
            Meaning::SyntaxComment => self.comment,
            _ => self.base,
        }
    }
}

fn default_theme(mgr: &mut ThemeManager) -> &Theme {
    mgr.load_theme("default", None)
}

/// OLD: probe the theme's `HashMap` for every character.
#[divan::bench(min_time = 1)]
fn per_char_as_style(bencher: divan::Bencher) {
    let mut mgr = ThemeManager::new(Some(false), Some(String::new()));
    let theme = default_theme(&mut mgr);
    bencher.with_inputs(frame).bench_values(|rows: Vec<Vec<Meaning>>| {
        for row in &rows {
            for &meaning in row {
                divan::black_box(theme.as_style(meaning));
            }
        }
    });
}

/// NEW: resolve the syntax meanings once per row, then match per character.
#[divan::bench(min_time = 1)]
fn per_row_style_table(bencher: divan::Bencher) {
    let mut mgr = ThemeManager::new(Some(false), Some(String::new()));
    let theme = default_theme(&mut mgr);
    bencher.with_inputs(frame).bench_values(|rows: Vec<Vec<Meaning>>| {
        for row in &rows {
            // `black_box` the theme so the loop-invariant table build cannot be
            // hoisted out of the per-row loop, matching the real per-row cost.
            let table = SyntaxStyles::resolve(divan::black_box(theme));
            for &meaning in row {
                divan::black_box(table.get(meaning));
            }
        }
    });
}
