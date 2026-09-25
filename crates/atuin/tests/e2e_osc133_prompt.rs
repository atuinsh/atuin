//! OSC 133 prompt wrapping in the bash and zsh init scripts.
//!
//! `__atuin_osc133_wrap_prompt` runs on every prompt and reads back the prompt it wrote the
//! previous time, so a bug that appends where it should replace is invisible on the first call and
//! only appears from the second one on. Every case here therefore drives the function repeatedly
//! and asserts the prompt reaches a fixed point, rather than checking a single invocation.
//!
//! These source the real init scripts in a non-interactive shell and call the function directly:
//! the wrapping is pure string handling over `PS1`/`PROMPT`, so it needs no terminal, and keeping
//! it out of the PTY suite makes the marker assertions exact instead of screen-scraped.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;

use rstest::rstest;

/// Source the init script, force the proxy state under test, then drive the prompt hook.
///
/// The hook is called more than once because it is not a pure function of its input: it reads the
/// prompt it wrote last time, and the second call is the first one that can observe the difference.
const DRIVER: &str = r#"
source "$ATUIN_TEST_INIT"

__atuin_pty_proxy_owns_tty=$ATUIN_TEST_OWNS_TTY
if [ "$ATUIN_TEST_PROXY_ACTIVE" = 1 ]; then
    ATUIN_PTY_PROXY_ACTIVE=1
else
    unset ATUIN_PTY_PROXY_ACTIVE
fi

unset RPROMPT RPS1
if [ -n "${ATUIN_TEST_RPROMPT+x}" ]; then
    RPROMPT=$ATUIN_TEST_RPROMPT
fi
# zsh's PROMPT and PS1 name the same parameter, so one assignment serves both scripts.
PS1=$ATUIN_TEST_PROMPT

for ((i = 0; i < ATUIN_TEST_ITERATIONS; i++)); do
    __atuin_osc133_wrap_prompt
    printf '%s\0%s\0%s\0' "$PS1" "${RPROMPT+1}" "${RPROMPT-}"
done
"#;

/// How many times each case drives the hook. Three is one more than the minimum that can catch a
/// non-idempotent write, so a prompt that grows by a fixed amount each time is unambiguous.
const ITERATIONS: usize = 3;

#[derive(Clone, Copy)]
struct Shell {
    name: &'static str,
    args: &'static [&'static str],
    init: &'static str,
    start: &'static str,
    end: &'static str,
    /// Whether the end marker lives in the right prompt rather than beside the start marker.
    splits_rprompt: bool,
}

impl Shell {
    /// Lay Atuin's markers around `inner` the way this script does, so a case can start from a
    /// prompt that really could have been inherited from a session where the proxy owned the tty.
    fn wrapped(self, inner: &str) -> (String, Option<String>) {
        if self.splits_rprompt {
            (format!("{}{inner}", self.start), Some(self.end.to_owned()))
        } else {
            (format!("{}{inner}{}", self.start, self.end), None)
        }
    }
}

const BASH: Shell = Shell {
    name: "bash",
    args: &["--norc", "--noprofile"],
    init: concat!(env!("CARGO_MANIFEST_DIR"), "/src/shell/atuin.bash"),
    start: "\x01\x1b]133;A;cl=line\x07\x02",
    end: "\x01\x1b]133;B\x07\x02",
    splits_rprompt: false,
};

const ZSH: Shell = Shell {
    name: "zsh",
    args: &["-f"],
    init: concat!(env!("CARGO_MANIFEST_DIR"), "/src/shell/atuin.zsh"),
    start: "%{\x1b]133;A;cl=line\x07%}",
    end: "%{\x1b]133;B\x07%}",
    splits_rprompt: true,
};

/// One call's worth of prompt state. `right` is `None` while `RPROMPT` has never been assigned,
/// which zsh tracks separately from its value (#3758) and the hook is careful not to disturb.
#[derive(Clone, PartialEq, Eq)]
struct Frame {
    left: String,
    right: Option<String>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "left={:?}", self.left)?;
        match &self.right {
            Some(right) => write!(f, " right={right:?}"),
            None => write!(f, " right=<unassigned>"),
        }
    }
}

struct Case {
    owns_tty: bool,
    proxy_active: bool,
    prompt: String,
    rprompt: Option<String>,
}

impl Case {
    /// The proxy owns the terminal, so Atuin is the one emitting OSC 133 and wraps the prompt.
    fn owning(prompt: impl Into<String>) -> Self {
        Self {
            owns_tty: true,
            proxy_active: true,
            prompt: prompt.into(),
            rprompt: None,
        }
    }

    /// A proxy is in the environment but does not own this terminal: Atuin adds no markers and
    /// only cleans up its own, e.g. ones inherited through `export PS1`.
    fn not_owning(prompt: impl Into<String>) -> Self {
        Self {
            owns_tty: false,
            proxy_active: true,
            prompt: prompt.into(),
            rprompt: None,
        }
    }

    /// No proxy anywhere. Atuin has no business touching the prompt at all.
    fn no_proxy(prompt: impl Into<String>) -> Self {
        Self {
            owns_tty: false,
            proxy_active: false,
            prompt: prompt.into(),
            rprompt: None,
        }
    }

    fn rprompt(mut self, rprompt: Option<String>) -> Self {
        self.rprompt = rprompt;
        self
    }

    /// Returns `None` when the shell is not installed, matching the rest of the shell suite.
    fn run(&self, shell: Shell) -> Option<Vec<Frame>> {
        let executable = find(shell.name)?;
        let bin = PathBuf::from(env!("CARGO_BIN_EXE_atuin"));
        let path = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).fold(
            bin.parent().unwrap().to_path_buf().into_os_string(),
            |mut acc, dir| {
                acc.push(":");
                acc.push(dir);
                acc
            },
        );

        let mut command = Command::new(&executable);
        command
            .args(shell.args)
            .arg("-c")
            .arg(DRIVER)
            .env("PATH", path)
            .env("ATUIN_TEST_INIT", shell.init)
            .env(
                "ATUIN_TEST_OWNS_TTY",
                if self.owns_tty {
                    "1"
                } else {
                    "0"
                },
            )
            .env(
                "ATUIN_TEST_PROXY_ACTIVE",
                if self.proxy_active {
                    "1"
                } else {
                    "0"
                },
            )
            .env("ATUIN_TEST_PROMPT", &self.prompt)
            .env("ATUIN_TEST_ITERATIONS", ITERATIONS.to_string())
            .env("ATUIN_UPDATE_CHECK", "false")
            .env_remove("ATUIN_TEST_RPROMPT");
        if let Some(rprompt) = &self.rprompt {
            command.env("ATUIN_TEST_RPROMPT", rprompt);
        }

        let output = command.output().expect("failed to run shell");
        assert!(
            output.status.success(),
            "{} exited {:?}\nstderr: {}",
            executable.display(),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
        );

        // The driver separates fields with NUL so a prompt may hold any byte a prompt can hold.
        let mut fields = output.stdout.split(|byte| *byte == 0).map(String::from_utf8_lossy);
        let frames = (0..ITERATIONS)
            .map(|_| {
                let left = fields.next().expect("driver produced too few fields").into_owned();
                let assigned = fields.next().expect("driver produced too few fields");
                let right = fields.next().expect("driver produced too few fields").into_owned();
                Frame {
                    left,
                    right: (!assigned.is_empty()).then_some(right),
                }
            })
            .collect();
        Some(frames)
    }
}

fn find(shell: &str) -> Option<PathBuf> {
    let override_var = format!("ATUIN_E2E_{}", shell.to_uppercase());
    if let Some(path) = std::env::var_os(&override_var) {
        let path = PathBuf::from(path);
        assert!(path.is_file(), "{override_var} is not a file: {}", path.display());
        return Some(path);
    }
    let found = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(shell))
        .find(|path| path.is_file());
    if found.is_none() {
        assert!(std::env::var_os("ATUIN_E2E_REQUIRE_SHELLS").is_none(), "missing shell: {shell}");
        eprintln!("skipping: missing shell: {shell}");
    }
    found
}

/// The hook must be a fixed point from its first call: whatever it makes of a prompt, feeding that
/// back in has to leave it alone. This is the assertion the repeated calls exist for.
#[track_caller]
fn assert_settled(frames: &[Frame]) {
    for (i, frame) in frames.iter().enumerate().skip(1) {
        assert_eq!(
            frame, &frames[0],
            "call {} changed the prompt again; the hook is not idempotent\n  first: {:?}\n  now:   {frame:?}",
            i + 1,
            frames[0],
        );
    }
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

#[rstest]
fn a_clean_prompt_is_wrapped_in_exactly_one_pair_of_markers(#[values(BASH, ZSH)] shell: Shell) {
    let Some(frames) = Case::owning("$ ").run(shell) else {
        return;
    };
    assert_settled(&frames);

    let Frame { left, right } = &frames[0];
    // bash keeps both markers in `PS1`; zsh splits them across `PROMPT` and `RPROMPT`.
    let whole = format!("{left}{}", right.as_deref().unwrap_or_default());
    assert_eq!(count(&whole, shell.start), 1, "{} start markers in {whole:?}", shell.name);
    assert_eq!(count(&whole, shell.end), 1, "{} end markers in {whole:?}", shell.name);
    assert!(whole.contains("$ "), "the user's own prompt was lost: {whole:?}");
}

/// Regression: the hook used to restore the *already wrapped* prompt when it found a marker it
/// had not written, then wrap that again, so the prompt grew by one marker pair on every prompt
/// for the rest of the session.
#[rstest]
fn a_foreign_marker_does_not_make_the_prompt_grow(#[values(BASH, ZSH)] shell: Shell) {
    let foreign = "\x1b]133;A\x07";
    let Some(frames) = Case::owning(format!("{foreign}$ ")).run(shell) else {
        return;
    };
    assert_settled(&frames);

    let Frame { left, right } = &frames[0];
    let whole = format!("{left}{}", right.as_deref().unwrap_or_default());
    assert_eq!(count(&whole, shell.start), 1, "{} start markers in {whole:?}", shell.name);
    assert_eq!(count(&whole, shell.end), 1, "{} end markers in {whole:?}", shell.name);
    assert!(whole.contains(foreign), "the other program's marker survived: {whole:?}");
}

#[rstest]
fn markers_inherited_into_a_shell_the_proxy_does_not_own_are_stripped(
    #[values(BASH, ZSH)] shell: Shell,
) {
    let (prompt, rprompt) = shell.wrapped("$ ");
    let stripped = rprompt.as_ref().map(|_| String::new());
    let Some(frames) = Case::not_owning(prompt).rprompt(rprompt).run(shell) else {
        return;
    };
    assert_settled(&frames);

    assert_eq!(frames[0].left, "$ ", "{}: markers left behind", shell.name);
    assert_eq!(frames[0].right, stripped, "{}: rprompt not stripped", shell.name);
}

/// With a marker Atuin did not write still present, a removal that did match was likely a
/// coincidence, so the hook leaves both prompts exactly as it found them rather than risk
/// removing one half of another program's pair.
#[rstest]
fn a_prompt_holding_a_foreign_marker_is_left_untouched(#[values(BASH, ZSH)] shell: Shell) {
    let (wrapped, rprompt) = shell.wrapped("$ ");
    let prompt = format!("\x1b]133;A\x07{wrapped}");
    let case = Case::not_owning(prompt.clone()).rprompt(rprompt.clone());
    let Some(frames) = case.run(shell) else {
        return;
    };
    assert_settled(&frames);

    assert_eq!(frames[0].left, prompt, "{}: the prompt was modified", shell.name);
    assert_eq!(frames[0].right, rprompt, "{}: the rprompt was modified", shell.name);
}

#[rstest]
fn no_proxy_means_the_prompt_is_never_touched(#[values(BASH, ZSH)] shell: Shell) {
    let prompt = format!("{}$ {}", shell.start, shell.end);
    let Some(frames) = Case::no_proxy(prompt.clone()).run(shell) else {
        return;
    };
    assert_settled(&frames);

    assert_eq!(frames[0].left, prompt, "{}: the prompt was modified", shell.name);
}

/// Assigning `RPROMPT` marks it, and the `RPS1` it shares a buffer with, as set (#3758). The hook
/// must not do that just by running.
#[rstest]
fn zsh_leaves_rprompt_unassigned_when_there_is_nothing_to_strip() {
    let Some(frames) = Case::not_owning("$ ").run(ZSH) else {
        return;
    };
    assert_settled(&frames);

    assert_eq!(frames[0].right, None, "the hook assigned RPROMPT with nothing to strip");
}

/// bash expands `\e` and `\033` in `PS1` itself, so both spell the same escape once the prompt is
/// drawn and both have to count as a foreign marker.
#[rstest]
fn bash_recognises_every_spelling_of_a_foreign_escape(
    #[values("\x1b", r"\e", r"\033")] escape: &str,
) {
    let prompt = format!(r"\[{escape}]133;A\a\]{}$ ", BASH.start);
    let Some(frames) = Case::not_owning(prompt.clone()).run(BASH) else {
        return;
    };
    assert_settled(&frames);

    assert_eq!(frames[0].left, prompt, "{escape:?} was not recognised as a marker");
}
