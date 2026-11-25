//! Runs login files in Bash, Zsh, and dash before and after setup adds its block, and checks that
//! the block changes nothing that the file does.
//!
//! Wherever setup puts the block, the edited file has to parse as the original does, and its
//! `exec` has to run under the same condition. When the `exec` runs, the block has to run before
//! it, or setup has to say that the block comes after the `exec`.

use std::path::{Path, PathBuf};

use proptest::prelude::*;
use rstest::rstest;

use super::super::super::super::exec::{ExecPlacement, statement_before_exec};
use super::{
    AFTER_THE_BLOCK, LoginHome, LoginShell, Placement, Syntax, bash_major_version, home, with_line,
};

/// What the scan makes of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scan {
    Before,
    Unknown,
    NoExec,
}

impl Scan {
    fn of(text: &str) -> Self {
        match statement_before_exec(text.as_bytes()) {
            ExecPlacement::Before(_) => Self::Before,
            ExecPlacement::Unknown { .. } => Self::Unknown,
            ExecPlacement::NoExec => Self::NoExec,
        }
    }

    fn named(name: &str) -> Self {
        match name {
            "before" => Self::Before,
            "unknown" => Self::Unknown,
            "no-exec" => Self::NoExec,
            _ => panic!("unknown scan result {name:?}"),
        }
    }
}

/// A file of the corpus in `tests/login_files`.
///
/// The file starts with the line `# shells: …`, which names the shells that run the file without
/// an error, and the line `# scan: …`, which says what the scan makes of the file: `before`,
/// `unknown`, or `no-exec`.
struct CorpusFile {
    text: String,
}

impl CorpusFile {
    fn read(path: &Path) -> Self {
        let text = std::fs::read_to_string(path).unwrap();
        Self { text }
    }

    /// The value of the header line `# {name}: …`.
    fn header(&self, name: &str) -> &str {
        self.text
            .lines()
            .find_map(|line| line.strip_prefix("# ")?.strip_prefix(name)?.strip_prefix(':'))
            .unwrap_or_else(|| panic!("the file needs a line `# {name}: …`"))
            .trim()
    }

    fn accepted_by(&self, shell: LoginShell) -> bool {
        self.header("shells").split_whitespace().any(|name| name == shell.program())
    }

    fn scan(&self) -> Scan {
        Scan::named(self.header("scan"))
    }
}

/// What a shell did when it ran a file as a script.
#[derive(Debug)]
struct Run {
    output: std::process::Output,
    /// What the program of the `exec` recorded, or `None` when the `exec` didn't run.
    exec: Option<String>,
}

impl Run {
    /// Whether the shell ran the whole file without an error.
    fn accepted(&self) -> bool {
        self.output.status.success() && self.output.stderr.is_empty()
    }
}

/// A file's run before and after setup added its block.
#[derive(Debug)]
struct Runs {
    original: Run,
    edited: Run,
    placement: Placement,
}

impl LoginHome {
    /// Runs `script` in `shell` with `XDG_VTNR` set to `vtnr`. `None` means that `shell` isn't
    /// installed.
    fn run(&self, shell: LoginShell, script: &[u8], vtnr: &str) -> Option<Run> {
        for record in ["ran", "wm-ran"] {
            let _ = std::fs::remove_file(self.path(record));
        }
        let path = self.path("script");
        std::fs::write(&path, script).unwrap();
        let output = std::process::Command::new(shell.program())
            .arg(&path)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("PATH", format!("{}:/usr/bin:/bin", self.shown("bin")))
            .env("TERM", "linux")
            .env("XDG_VTNR", vtnr)
            .output()
            .ok()?;
        let exec = std::fs::read_to_string(self.path("wm-ran")).ok();
        Some(Run { output, exec })
    }

    /// Runs `text` in `shell` as it is, and with setup's block.
    fn runs(&self, shell: LoginShell, text: &str, vtnr: &str) -> Option<Runs> {
        let original = self.run(shell, text.as_bytes(), vtnr)?;
        let env = std::collections::BTreeMap::new();
        let atuin = self.shown("atuin");
        let line = super::super::super::super::syntax::Launch {
            atuin: &atuin,
            env: &env,
        }
        .sh_line();
        let placed = with_line(text.as_bytes(), &line, Syntax::Sh);
        let edited = self.run(shell, &placed.text, vtnr)?;
        Some(Runs {
            original,
            edited,
            placement: placed.placement,
        })
    }
}

/// Checks that the block changed nothing that the original file did, and that the block runs
/// before the `exec` unless setup says otherwise.
fn assert_the_block_keeps_the_run(runs: &Runs) {
    let Runs {
        original,
        edited,
        placement,
    } = runs;
    assert!(original.accepted(), "the shell runs the original: {original:?}");
    assert!(edited.accepted(), "the shell runs the edited file: {edited:?}");
    assert_eq!(edited.output.stdout, original.output.stdout, "the output stays");
    assert_eq!(
        edited.exec.is_some(),
        original.exec.is_some(),
        "the exec runs under its condition: {runs:?}"
    );
    if let Some(exec) = &edited.exec {
        assert!(
            exec.trim_end() == AFTER_THE_BLOCK || *placement == Placement::AfterExec,
            "the block runs before the exec, or setup says that it doesn't: {runs:?}"
        );
    }
}

/// Setup's block keeps what each file of the corpus does, in each shell that runs the file.
#[rstest]
fn the_block_keeps_what_a_corpus_file_does(
    home: LoginHome,
    #[files("tests/login_files/*.sh")] path: PathBuf,
    #[values(LoginShell::Bash, LoginShell::Zsh, LoginShell::Dash)] shell: LoginShell,
    #[values("1", "2")] vtnr: &str,
) {
    let file = CorpusFile::read(&path);
    assert_eq!(Scan::of(&file.text), file.scan(), "the scan of {path:?}");
    let Some(runs) = home.runs(shell, &file.text, vtnr) else {
        return;
    };
    let accepted = runs.original.accepted();
    // The corpus names the shells that accept each file as Bash 4 or later does. Bash 3.2, which
    // macOS ships, rejects some of those files, so the check of the edited file then covers only
    // the files that Bash 3.2 accepts.
    let old_bash =
        matches!(shell, LoginShell::Bash) && bash_major_version().is_some_and(|major| major < 4);
    if !old_bash {
        assert_eq!(
            accepted,
            file.accepted_by(shell),
            "{shell:?} runs {path:?}: {:?}",
            runs.original
        );
    }
    if accepted {
        assert_the_block_keeps_the_run(&runs);
    }
}

/// Statements of `sh` that the scan follows, and that come before or after the `exec`.
static STATEMENTS: [&str; 16] = [
    "export A=1\n",
    "case \"$TERM\" in\n  linux) A=1 ;;\n  *) A=2 ;;\nesac\n",
    "case x in (x) case y in y) : ;; esac ;; esac\n",
    "x=$(case a in a) echo b;; esac)\n",
    "y=\"$(echo \")\")\"\n",
    "cat >/dev/null <<EOF\nexec sway\n)\nEOF\n",
    "cat >/dev/null <<-'EOF'\n\tcase\n\tEOF\n",
    "z=$(( 1 << 2 ))\n",
    "f() {\n  :\n}\n",
    "if [ -n \"$HOME\" ]; then\n  :\nfi\n",
    "for i in 1 2; do\n  case $i in 1) continue ;; esac\ndone\n",
    "y=`echo a`\n",
    "# A note about exec sway.\n",
    "x=$(cat <<EOF\nit's )\nEOF\n)\n",
    "echo \"exec sway\" >/dev/null\n",
    "eval \"$(echo export B=1)\"\n",
];

/// Statements of `sh` that replace the shell on the first console.
static EXECS: [&str; 12] = [
    "[ \"$XDG_VTNR\" = 1 ] && exec sway\n",
    "[ \"$XDG_VTNR\" = 1 ] &&\n  exec sway\n",
    "if [ \"$XDG_VTNR\" = 1 ]; then\n  exec sway\nfi\n",
    "if [ \"$XDG_VTNR\" = 1 ]\nthen exec sway\nfi\n",
    "case \"$XDG_VTNR\" in\n  1) exec sway ;;\nesac\n",
    "case $XDG_VTNR in (1) exec startx;; esac\n",
    "[ \"$XDG_VTNR\" != 1 ] || exec sway\n",
    "start() {\n  exec sway\n}\n[ \"$XDG_VTNR\" = 1 ] && start\n",
    "[ \"$XDG_VTNR\" = 1 ] && {\n  exec sway\n}\n",
    "while [ \"$XDG_VTNR\" = 1 ]; do exec sway; done\n",
    "[ \"$XDG_VTNR\" = 1 ] && WLR_NO_HARDWARE_CURSORS=1 exec sway\n",
    "[ \"$XDG_VTNR\" = 1 ] && exec 2>/dev/null sway\n",
];

/// A login file of statements from [`STATEMENTS`] around a statement from [`EXECS`].
fn login_file() -> impl Strategy<Value = String> {
    let statements = || prop::collection::vec(prop::sample::select(&STATEMENTS[..]), 0..4);
    (statements(), prop::sample::select(&EXECS[..]), statements()).prop_map(
        |(before, exec, after)| {
            let mut file: String = before.concat();
            file.push_str(exec);
            file.extend(after);
            file.push_str("echo END\n");
            file
        },
    )
}

proptest! {
    /// The scan places the block before the `exec` of a file that it follows, and setup's edit
    /// is undone by a removal and stays the same in a second run.
    #[rstest]
    fn the_scan_places_the_block_in_a_followed_file(file in login_file()) {
        prop_assert_eq!(Scan::of(&file), Scan::Before);
        let placed = with_line(file.as_bytes(), super::LINE, Syntax::Sh);
        prop_assert_eq!(super::without_line(&placed.text), file.as_bytes());
        prop_assert_eq!(with_line(&placed.text, super::LINE, Syntax::Sh).text, placed.text);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Setup's block keeps what a generated file does, in each shell.
    #[rstest]
    fn the_block_keeps_what_a_generated_file_does(
        file in login_file(),
        vtnr in prop::sample::select(vec!["1", "2"]),
    ) {
        let home = home();
        for shell in [LoginShell::Bash, LoginShell::Zsh, LoginShell::Dash] {
            if let Some(runs) = home.runs(shell, &file, vtnr) {
                // Bash 3.2, which macOS ships, rejects some of the generated files, and the
                // check then covers only the files that Bash 3.2 accepts.
                let old_bash = matches!(shell, LoginShell::Bash)
                    && bash_major_version().is_some_and(|major| major < 4);
                if !old_bash || runs.original.accepted() {
                    assert_the_block_keeps_the_run(&runs);
                }
            }
        }
    }
}
