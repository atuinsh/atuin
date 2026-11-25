//! Login shells that run login files with setup's block, on the platforms that have them.

use std::path::PathBuf;

use rstest::{fixture, rstest};

use super::*;

/// A shell that runs a login file.
#[derive(Debug, Clone, Copy)]
enum LoginShell {
    Bash,
    Zsh,
    Dash,
}

impl LoginShell {
    const fn program(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::Dash => "dash",
        }
    }

    /// The login file that the shell reads first.
    const fn file(self) -> &'static str {
        match self {
            Self::Bash => ".bash_profile",
            Self::Zsh => ".zprofile",
            Self::Dash => ".profile",
        }
    }
}

/// The directories where the login shells of the tests find the system's programs.
const SYSTEM_PATH: &str = "/usr/bin:/bin";

/// A home for login shells, with a stub of `atuin` that records the `MY_SETTING` that the
/// block saw, and the programs `fake-wm`, `sway`, and `startx`, which record that they ran.
struct LoginHome {
    dir: tempfile::TempDir,
}

/// What [`LoginHome`]'s programs record in `wm-ran` when the block ran before them.
const AFTER_THE_BLOCK: &str = "after the block";

#[fixture]
fn home() -> LoginHome {
    let dir = tempfile::tempdir().unwrap();
    let home = LoginHome { dir };
    let bin = home.path("bin");
    std::fs::create_dir(&bin).unwrap();
    let ran = home.shown("ran");
    home.write_script("atuin", &format!("printf %s \"$MY_SETTING\" > '{ran}'"));
    let wm = format!(
        "if [ -e '{ran}' ]; then echo '{AFTER_THE_BLOCK}'; else echo alone; fi > '{}'",
        home.shown("wm-ran")
    );
    for program in ["fake-wm", "sway", "startx"] {
        home.write_script(&format!("bin/{program}"), &wm);
    }
    home
}

/// What a login shell did with a login file.
struct Login {
    output: std::process::Output,
    placement: Placement,
}

impl LoginHome {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn shown(&self, name: &str) -> String {
        self.path(name).display().to_string()
    }

    fn write_script(&self, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt as _;

        let path = self.path(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Writes `profile`, with setup's block, as the login file of `shell`, and runs `shell` as a
    /// login shell with `XDG_VTNR` set to `vtnr`.
    ///
    /// `None` means that `shell` isn't installed.
    fn log_in(&self, shell: LoginShell, profile: &str, vtnr: &str) -> Option<Login> {
        let env = std::collections::BTreeMap::new();
        let atuin = self.shown("atuin");
        let line = super::super::super::syntax::Launch {
            atuin: &atuin,
            env: &env,
        }
        .sh_line();
        let placed = with_line(profile.as_bytes(), &line, Syntax::Sh);
        std::fs::write(self.path(shell.file()), placed.text).unwrap();
        let output = std::process::Command::new(shell.program())
            .args(["-l", "-c", "true"])
            .env_clear()
            .env("HOME", self.dir.path())
            .env("ZDOTDIR", self.dir.path())
            .env("PATH", format!("{}:{SYSTEM_PATH}", self.shown("bin")))
            .env("XDG_VTNR", vtnr)
            .output()
            .ok()?;
        Some(Login {
            output,
            placement: placed.placement,
        })
    }

    /// What the stub of `atuin` recorded, or `None` when the block didn't run.
    fn block_saw(&self) -> Option<String> {
        std::fs::read_to_string(self.path("ran")).ok()
    }

    fn exists(&self, name: &str) -> bool {
        self.path(name).exists()
    }
}

/// A Bash login shell runs the block, and the block sees what the file exported before it.
///
/// The files end by replacing the shell, as a console login that starts a compositor does,
/// or they only redirect with `exec`. `XDG_VTNR=2` keeps the functions from running.
#[rstest]
#[case::exec_at_the_end("export MY_SETTING=seen\nexec true\n")]
#[case::exec_after_an_assignment("export MY_SETTING=seen\nWLR_NO_HARDWARE_CURSORS=1 exec true\n")]
#[case::function_header_on_its_own_line(
    "export MY_SETTING=seen\nstart_wm()\n{\n  exec true\n}\n[ \"$XDG_VTNR\" = 1 ] && start_wm\n"
)]
#[case::function_keyword(
    "export MY_SETTING=seen\nfunction start_wm {\n  exec true\n}\n[ \"$XDG_VTNR\" = 1 ] && \
     start_wm\n"
)]
#[case::exec_that_only_redirects("exec 2>/dev/null\nexport MY_SETTING=seen\n")]
#[case::ansi_c_quote("x=$'it\\'s'\nexport MY_SETTING=seen\nexec true\n")]
#[case::shift_in_a_multi_line_substitution(
    "x=$(\n  y=$(( 1 << 2 ))\n  echo $y\n)\nexport MY_SETTING=seen\nexec true\n"
)]
fn bash_runs_the_block_after_the_exports(home: LoginHome, #[case] profile: &str) {
    login_runs_the_block_after_the_exports(&home, LoginShell::Bash, profile);
}

/// A Bash login shell runs the block after a here document with a quote in a command
/// substitution.
///
/// Bash 3.2, which macOS ships, can't parse such a file at all, so the test needs Bash 4 or
/// later. The test says so on stderr when the test skips the check.
#[rstest]
fn bash_runs_the_block_after_a_here_document_in_a_substitution(home: LoginHome) {
    match bash_major_version() {
        Some(major) if major >= 4 => {}
        version => {
            assert!(
                version.is_some() || !cfg!(target_os = "linux"),
                "bash must be installed to run the login tests on Linux"
            );
            eprintln!("skipped: the test needs bash 4 or later, and found {version:?}");
            return;
        }
    }
    let profile = "x=$(cat <<EOF\nit's\nEOF\n)\nexport MY_SETTING=seen\nexec true\n";
    login_runs_the_block_after_the_exports(&home, LoginShell::Bash, profile);
}

/// The major version of the Bash that [`LoginHome::log_in`] runs, or `None` when Bash isn't
/// installed.
fn bash_major_version() -> Option<u32> {
    let output = std::process::Command::new(LoginShell::Bash.program())
        .args(["-c", "echo \"${BASH_VERSINFO[0]}\""])
        .env("PATH", SYSTEM_PATH)
        .output()
        .ok()?;
    std::str::from_utf8(&output.stdout).ok()?.trim().parse().ok()
}

/// A Zsh login shell runs the block in a `.zprofile` with Zsh's short forms.
#[rstest]
#[case::short_function("f() { echo hi }\nexport MY_SETTING=seen\nexec true\n")]
#[case::short_if("if [[ -o login ]] { true }\nexport MY_SETTING=seen\nexec true\n")]
#[case::short_for("for x (a b) { true }\nexport MY_SETTING=seen\nexec true\n")]
#[case::exec_in_a_short_if("export MY_SETTING=seen\nif [[ -o login ]] { exec true }\n")]
fn zsh_runs_the_block_after_the_exports(home: LoginHome, #[case] profile: &str) {
    login_runs_the_block_after_the_exports(&home, LoginShell::Zsh, profile);
}

/// Runs `shell` as a login shell on `profile`, with setup's block in it, and checks that the
/// block ran and saw `MY_SETTING`.
fn login_runs_the_block_after_the_exports(home: &LoginHome, shell: LoginShell, profile: &str) {
    let Some(login) = home.log_in(shell, profile, "2") else {
        return;
    };

    assert!(login.output.status.success(), "{:?}", login.output);
    assert_eq!(home.block_saw().as_deref(), Some("seen"), "the block ran after the export");
}

/// What comes before the statement of the `exec` in a login file.
#[derive(Debug, Clone, Copy)]
enum Preamble {
    Plain,
    /// A `case` whose patterns are reserved words.
    ReservedWordPatterns,
    /// A `case` inside a command substitution, with patterns that lack the leading `(`.
    CaseInASubstitution,
    /// A test of Bash and Zsh, `[[ … ]]`, that spans two lines.
    MultiLineTest,
    /// A coprocess of Bash and Zsh, which the scan doesn't follow.
    Coprocess,
}

impl Preamble {
    const fn text(self) -> &'static str {
        match self {
            Self::Plain => "",
            Self::ReservedWordPatterns => {
                "case \"$TERM\" in\n  done|fi) echo odd ;;\n  (esac) : ;;\n  (for) echo ;;\nesac\n"
            }
            Self::CaseInASubstitution => "x=$( case a in a) echo b;; esac )\n",
            Self::MultiLineTest => "[[ -n $HOME\n  && $HOME =~ (for|fi|/) ]] && :\n",
            Self::Coprocess => "coproc true\n",
        }
    }
}

/// Regardless of what setup makes of a login file, the `exec` runs only when its condition
/// holds, and the shell reads the whole file.
///
/// The `exec` replaces the login shell with `fake-wm` on the first console. With `XDG_VTNR=2`,
/// the login shell has to run the block and reach the end of the file. With `XDG_VTNR=1`,
/// `fake-wm` has to run.
#[rstest]
#[case::and_and("[ \"$XDG_VTNR\" = 1 ] && exec fake-wm\n")]
#[case::and_and_on_two_lines("[ \"$XDG_VTNR\" = 1 ] &&\n  exec fake-wm\n")]
#[case::or_or("[ \"$XDG_VTNR\" != 1 ] ||\n  exec fake-wm\n")]
#[case::then_on_the_line_of_the_exec("if [ \"$XDG_VTNR\" = 1 ]\nthen exec fake-wm\nfi\n")]
#[case::then_on_its_own_line("if [ \"$XDG_VTNR\" = 1 ]\nthen\n  exec fake-wm\nfi\n")]
#[case::else_on_the_line_of_the_exec("if [ \"$XDG_VTNR\" != 1 ]\nthen :\nelse exec fake-wm\nfi\n")]
#[case::elif(
    "if [ \"$XDG_VTNR\" = 0 ]\nthen :\nelif [ \"$XDG_VTNR\" = 1 ]\nthen exec fake-wm\nfi\n"
)]
#[case::a_case_arm("case \"$XDG_VTNR\" in\n  1) exec fake-wm ;;\nesac\n")]
#[case::a_case_arm_on_its_own_lines(
    "case \"$XDG_VTNR\" in\n  1)\n    exec fake-wm\n    ;;\nesac\n"
)]
#[case::a_case_arm_with_parens("case \"$XDG_VTNR\" in\n  (1) exec fake-wm ;;\nesac\n")]
#[case::a_case_arm_after_reserved_word_patterns(
    "case \"$XDG_VTNR\" in\n  done|fi) echo odd ;;\n  (esac) : ;;\n  1) exec fake-wm ;;\nesac\n"
)]
#[case::do_while("while [ \"$XDG_VTNR\" = 1 ]\ndo exec fake-wm\ndone\n")]
#[case::do_for("for vt in 1\ndo [ \"$XDG_VTNR\" = \"$vt\" ] && exec fake-wm\ndone\n")]
#[case::braces("[ \"$XDG_VTNR\" = 1 ] && {\n  exec fake-wm\n}\n")]
#[case::braces_around_the_test("{\n  [ \"$XDG_VTNR\" = 1 ] &&\n  exec fake-wm\n}\n")]
#[case::a_subshell("[ \"$XDG_VTNR\" = 1 ] && (\n  exec fake-wm\n)\n")]
#[case::a_function("start_wm() {\n  exec fake-wm\n}\n[ \"$XDG_VTNR\" = 1 ] && start_wm\n")]
fn the_exec_runs_only_when_its_condition_holds(
    home: LoginHome,
    #[case] statement: &str,
    #[values(
        Preamble::Plain,
        Preamble::ReservedWordPatterns,
        Preamble::CaseInASubstitution,
        Preamble::MultiLineTest,
        Preamble::Coprocess
    )]
    preamble: Preamble,
    #[values(LoginShell::Bash, LoginShell::Zsh, LoginShell::Dash)] shell: LoginShell,
    #[values("1", "2")] vtnr: &str,
) {
    if matches!(
        (preamble, shell),
        (Preamble::MultiLineTest | Preamble::Coprocess, LoginShell::Dash)
    ) {
        return;
    }
    // Bash 3.2, which macOS ships, can't parse a `case` inside `$(…)` and doesn't know `coproc`.
    if matches!(
        (preamble, shell),
        (Preamble::CaseInASubstitution | Preamble::Coprocess, LoginShell::Bash)
    ) && bash_major_version().is_some_and(|major| major < 4)
    {
        return;
    }
    let end = home.shown("end");
    let profile = format!(
        "export MY_SETTING=seen\nPATH=\"$HOME/bin:$PATH\"\n{}{statement}: > '{end}'\n",
        preamble.text()
    );

    let Some(login) = home.log_in(shell, &profile, vtnr) else {
        return;
    };

    let stderr = String::from_utf8_lossy(&login.output.stderr);
    assert!(login.output.status.success(), "{:?}", login.output);
    assert!(!stderr.contains("error"), "the shell read the file: {stderr}");
    if vtnr == "1" {
        assert!(home.exists("wm-ran"), "the exec ran on the first console");
        return;
    }
    assert!(!home.exists("wm-ran"), "the exec didn't run on another console");
    assert!(home.exists("end"), "the shell reached the end of the file");
    assert_eq!(home.block_saw().as_deref(), Some("seen"), "the block ran");
    let expected = match preamble {
        Preamble::Plain
        | Preamble::ReservedWordPatterns
        | Preamble::CaseInASubstitution
        | Preamble::MultiLineTest => Placement::BeforeAnyExec,
        Preamble::Coprocess => Placement::AfterExec,
    };
    assert_eq!(login.placement, expected);
}

mod runs;
