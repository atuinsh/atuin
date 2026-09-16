//! Head-to-head for the search-result highlighter (`search/history_list.rs`).
//!
//! Every visible row is highlighted on every keystroke/frame. The old path
//! rebuilt the fuzzy scorer (`FzfV2::new` heap-allocates ~5kb) and the query
//! parser, and re-parsed the query, once *per row*. The new path builds the
//! scorer + parser once per frame and reuses them across rows.
//!
//! `atuin` is a binary crate with no lib target, so the two approaches are
//! replicated here against the same `norm` fzf engine the real engine uses
//! (`search/engines/db.rs`). The output of both is identical; this only
//! measures the setup that was hoisted out of the per-row loop.

use std::ops::Range;

use norm::Metric;
use norm::fzf::{FzfParser, FzfV2};

fn main() {
    divan::main();
}

/// 30 realistic shell commands — a typical visible page of history.
const COMMANDS: &[&str] = &[
    "git commit --amend --no-edit",
    "cargo test -p atuin-client --features sqlite",
    "docker compose up -d --build",
    "kubectl get pods -n production",
    "ssh deploy@10.0.0.4 'systemctl restart atuin'",
    "rg --hidden --glob '!.git' highlight crates/",
    "cargo build --release --target x86_64-unknown-linux-gnu",
    "git rebase --onto main feature/search-highlight-hoist",
    "psql -h localhost -U atuin -d atuin_dev",
    "tar czf backup.tar.gz ~/.local/share/atuin",
    "find . -name '*.rs' -newer Cargo.toml",
    "sed -i 's/foo/bar/g' src/main.rs",
    "npm run build && npm run test",
    "atuin search --cmd-only --limit 200 highlight",
    "curl -fsSL https://setup.atuin.sh | bash",
    "git log --oneline --graph --decorate -20",
    "cargo clippy --workspace --all-targets -- -D warnings",
    "ffmpeg -i input.mov -vcodec h264 output.mp4",
    "aws s3 sync ./dist s3://atuin-releases/latest",
    "grep -rn 'get_highlight_indices' crates/atuin/src",
    "systemctl --user status atuin-daemon.service",
    "python -m http.server 8080 --directory public",
    "git diff --stat origin/main HEAD",
    "brew upgrade && brew cleanup --prune=all",
    "cargo bench -p atuin --bench search_highlight",
    "tmux new-session -d -s work 'nvim .'",
    "jq '.results[] | select(.score > 0.9)' out.json",
    "rsync -avz --delete ./ backup:/srv/atuin/",
    "openssl x509 -in cert.pem -noout -text",
    "cargo run -- search --interactive fuzzy history",
];

/// A typical multi-token fuzzy query while hunting through history.
const QUERY: &str = "cargo test atuin";

/// OLD: build a fresh scorer + parser and re-parse the query for every row.
#[divan::bench]
fn old_per_row(bencher: divan::Bencher) {
    bencher.with_inputs(|| (COMMANDS, QUERY)).bench_values(|(commands, query)| {
        for command in commands {
            let mut fzf = FzfV2::new();
            let mut parser = FzfParser::new();
            let parsed = parser.parse(query);
            let mut ranges: Vec<Range<usize>> = Vec::new();
            let _ = fzf.distance_and_ranges(parsed, command, &mut ranges);
            divan::black_box(ranges.into_iter().flatten().collect::<Vec<usize>>());
        }
    });
}

/// NEW: build the scorer + parser once, reuse them across every row.
#[divan::bench]
fn new_hoisted(bencher: divan::Bencher) {
    bencher.with_inputs(|| (COMMANDS, QUERY)).bench_values(|(commands, query)| {
        let mut fzf = FzfV2::new();
        let mut parser = FzfParser::new();
        for command in commands {
            let parsed = parser.parse(query);
            let mut ranges: Vec<Range<usize>> = Vec::new();
            let _ = fzf.distance_and_ranges(parsed, command, &mut ranges);
            divan::black_box(ranges.into_iter().flatten().collect::<Vec<usize>>());
        }
    });
}
