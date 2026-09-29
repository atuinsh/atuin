//! Resuming a harness's session in the harness itself.
//!
//! A harness turns a [`ResumeTarget`] into a [`ResumePlan`] ([`Harness::resume`]): the program
//! and arguments that reopen the session, and where they must run. A plan renders to one shell
//! command, `cd -- '<cwd>' && <program> <args>...`, quoted so that zsh, bash and fish all read
//! it back as the same words ([`ResumePlan::render`]).
//!
//! [`Harness::resume`]: super::Harness::resume

use std::borrow::Cow;
use std::path::{Path, PathBuf};

/// A session to resume, as the harness recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeTarget {
    /// The harness's own id for the session.
    pub id: String,
    /// The directory the session last worked in, when known.
    pub cwd: Option<PathBuf>,
    /// The session's native transcript on this machine, when found (see
    /// [`Harness::locate`](super::Harness::locate)).
    pub native_path: Option<PathBuf>,
    /// Branch a copy of the session instead of continuing it in place.
    pub fork: bool,
}

impl ResumeTarget {
    /// A target for the session `id`, with nothing else known about it.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            cwd: None,
            native_path: None,
            fork: false,
        }
    }

    #[must_use]
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    #[must_use]
    pub fn with_native_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.native_path = Some(path.into());
        self
    }

    #[must_use]
    pub fn forked(mut self, fork: bool) -> Self {
        self.fork = fork;
        self
    }
}

/// Whether a plan has to run from the session's directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CwdRequirement {
    /// The harness cannot find or reopen the session from anywhere else.
    Required,
    /// The harness finds the session from anywhere, but works in (or asks about) the directory it
    /// runs from, so the session's own is the right one to be in.
    Preferred,
    /// Where the plan runs makes no difference.
    None,
}

/// How to reopen a session: `program` with `args`, run from `cwd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumePlan {
    pub program: String,
    pub args: Vec<String>,
    /// The directory to run from, when known.
    pub cwd: Option<PathBuf>,
    pub cwd_requirement: CwdRequirement,
    /// The session's native transcript, when the target named one.
    pub native_path: Option<PathBuf>,
}

impl ResumePlan {
    /// The plan as it stands on this machine: `Err` if it has to run from a directory that is
    /// unknown or gone, and without its `cwd` if it only prefers a directory that is gone.
    pub fn prepare(mut self) -> Result<Self, ResumeError> {
        let present = self.cwd.as_deref().is_some_and(Path::is_dir);
        match self.cwd_requirement {
            CwdRequirement::Required if !present => {
                return Err(ResumeError::CwdMissing(self.cwd));
            }
            CwdRequirement::Required => {}
            CwdRequirement::Preferred | CwdRequirement::None => {
                if !present {
                    self.cwd = None;
                }
            }
        }
        Ok(self)
    }

    /// The program and its arguments, quoted for the shell, without the `cd`.
    #[must_use]
    pub fn command(&self) -> String {
        let mut out = String::from(quote(&self.program));
        for arg in &self.args {
            out.push(' ');
            out.push_str(&quote(arg));
        }
        out
    }

    /// The plan as one shell command, `cd -- '<cwd>' && <command>` (just the command without a
    /// `cwd`). zsh, bash and fish all read it back as the same words.
    pub fn render(&self) -> Result<String, ResumeError> {
        let command = self.command();
        match &self.cwd {
            Some(cwd) => {
                let cwd = cwd.to_str().ok_or_else(|| ResumeError::NonUtf8Path(cwd.clone()))?;
                Ok(format!("cd -- {} && {command}", quote(cwd)))
            }
            None => Ok(command),
        }
    }

    /// Replace the program and arguments with the user's `template`, a command line whose
    /// `{id}`, `{path}` and `{cwd}` stand for the target's session id, native transcript path and
    /// directory.
    ///
    /// The template is split into words the way a POSIX shell would (so a literal word with a
    /// space in it can be quoted), and each placeholder is then substituted inside its word. A
    /// placeholder is therefore never quoted in the template itself: `--dir {cwd}` stays one
    /// word whatever the directory holds. A placeholder whose value is unknown is an error.
    pub fn with_template(
        mut self,
        template: &str,
        target: &ResumeTarget,
    ) -> Result<Self, ResumeError> {
        let words = shlex::split(template)
            .ok_or_else(|| ResumeError::InvalidTemplate("unbalanced quotes".into()))?;
        let mut words = words.into_iter().map(|word| substitute(&word, target));
        let program = words
            .next()
            .ok_or_else(|| ResumeError::InvalidTemplate("the template is empty".into()))??;
        self.program = program;
        self.args = words.collect::<Result<_, _>>()?;
        Ok(self)
    }
}

/// `word` with each placeholder replaced by its value in `target`.
fn substitute(word: &str, target: &ResumeTarget) -> Result<String, ResumeError> {
    let path_str = |name: &'static str, path: Option<&Path>| -> Result<String, ResumeError> {
        let path = path.ok_or(ResumeError::TemplateValueMissing(name))?;
        path.to_str().map(str::to_owned).ok_or_else(|| ResumeError::NonUtf8Path(path.to_path_buf()))
    };
    let mut out = String::with_capacity(word.len());
    let mut rest = word;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let tail = &rest[open..];
        let (value, len) = if tail.starts_with("{id}") {
            (target.id.clone(), "{id}".len())
        } else if tail.starts_with("{path}") {
            (path_str("path", target.native_path.as_deref())?, "{path}".len())
        } else if tail.starts_with("{cwd}") {
            (path_str("cwd", target.cwd.as_deref())?, "{cwd}".len())
        } else {
            ("{".to_owned(), 1)
        };
        out.push_str(&value);
        rest = &tail[len..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Why a session cannot be resumed here.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResumeError {
    #[error("{0}")]
    NotResumable(&'static str),

    #[error("the session's directory is {}", match .0 {
        Some(dir) => format!("gone: {}", dir.display()),
        None => "unknown".to_owned(),
    })]
    CwdMissing(Option<PathBuf>),

    #[error("the path is not valid UTF-8: {}", .0.display())]
    NonUtf8Path(PathBuf),

    #[error("the resume command template is invalid: {0}")]
    InvalidTemplate(String),

    #[error("the resume command template uses {{{0}}}, which this session does not have")]
    TemplateValueMissing(&'static str),
}

/// Quote `word` so that zsh, bash and fish all read it as that one word.
///
/// A word of only unremarkable characters is left bare. Anything else goes in single quotes,
/// inside which none of the three shells expands anything; the two characters fish still treats
/// specially there, `'` and `\`, are left outside the quotes and backslash-escaped, which reads
/// the same in all three: `it's` is `'it'\''s'`.
#[must_use]
pub fn quote(word: &str) -> Cow<'_, str> {
    let bare = |c: char| c.is_ascii_alphanumeric() || "_-./:@,+".contains(c);
    // `=` too, but not first: zsh expands a leading `=cmd` to the command's path.
    if !word.is_empty() && word.chars().all(|c| bare(c) || c == '=') && !word.starts_with('=') {
        return Cow::Borrowed(word);
    }
    let mut out = String::with_capacity(word.len() + 2);
    let mut open = false;
    for c in word.chars() {
        let special = c == '\'' || c == '\\';
        if open == special {
            // Close the quotes before a special character, reopen them after it.
            out.push('\'');
            open = !open;
        }
        if special {
            out.push('\\');
        }
        out.push(c);
    }
    if open {
        out.push('\'');
    }
    if out.is_empty() {
        out.push_str("''");
    }
    Cow::Owned(out)
}

/// A plan to run `program` with `args` for `target`, which needs its directory as `cwd`
/// describes.
pub(crate) fn plan(
    target: &ResumeTarget,
    cwd: CwdRequirement,
    program: &str,
    args: impl IntoIterator<Item = String>,
) -> ResumePlan {
    ResumePlan {
        program: program.to_owned(),
        args: args.into_iter().collect(),
        cwd: target.cwd.clone(),
        cwd_requirement: cwd,
        native_path: target.native_path.clone(),
    }
}

/// `path` as an argument, or `Err` if it is not UTF-8.
pub(crate) fn path_arg(path: &Path) -> Result<String, ResumeError> {
    path.to_str().map(str::to_owned).ok_or_else(|| ResumeError::NonUtf8Path(path.to_path_buf()))
}

/// Whether `id` can stand for a file name: not empty, no separator, not `.` or `..`. A session
/// id is looked up on disk by name, and one that is not a plain name must not reach outside the
/// harness's directory.
pub(crate) fn is_plain_name(id: &str) -> bool {
    !id.is_empty() && id != "." && id != ".." && !id.contains(['/', '\\', '\0'])
}

/// The first file under `root` (searched depth-first, symlinks not followed) that `accept`
/// takes.
pub(crate) fn find_file(root: &Path, accept: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() && accept(&path) {
                return Some(path);
            }
        }
    }
    None
}

/// Run a blocking file search off the async runtime.
pub(crate) async fn blocking<T: Send + 'static>(
    search: impl FnOnce() -> Option<T> + Send + 'static,
) -> Option<T> {
    tokio::task::spawn_blocking(search).await.ok().flatten()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::bare("abc-123_x.y/z", "abc-123_x.y/z")]
    #[case::uuid("0199aaaa-bbbb-4ccc-8ddd-eeeeffff0000", "0199aaaa-bbbb-4ccc-8ddd-eeeeffff0000")]
    #[case::empty("", "''")]
    #[case::space("/home/u/my project", "'/home/u/my project'")]
    #[case::single_quote("it's", r"'it'\''s'")]
    #[case::only_quote("'", r"\'")]
    #[case::backslash(r"a\b", r"'a'\\'b'")]
    #[case::double_backslash_quote(r"a\\'", r"'a'\\\\\'")]
    #[case::leading_equals("=ls", "'=ls'")]
    #[case::inner_equals("a=b", "a=b")]
    #[case::dollar("$HOME", "'$HOME'")]
    #[case::glob("*?[x]", "'*?[x]'")]
    #[case::tilde("~/x", "'~/x'")]
    #[case::unicode("/tmp/プロジェクト ü", "'/tmp/プロジェクト ü'")]
    #[case::newline("a\nb", "'a\nb'")]
    #[case::bang("hi!", "'hi!'")]
    fn quote_words(#[case] word: &str, #[case] expected: &str) {
        assert_eq!(quote(word), expected);
    }

    const NASTY: &[&str] = &[
        "plain",
        "",
        "/home/u/my project",
        "it's",
        r"back\slash",
        r"\\'\'",
        "$(rm -rf ~) `x` ${y}",
        "*?[a-z]{1,2}",
        "~user",
        "=cmd",
        "a;b|c&d>e<f",
        "tab\there",
        "new\nline",
        "\"double\"",
        "hi!!",
        "%self",
        "/tmp/プロジェクト/ü é/🦀",
        "-n",
        "#comment",
    ];

    const SHELLS: [&str; 3] = ["bash", "zsh", "fish"];

    /// Run `script` in `shell` without its startup files, returning its stdout; `None` when the
    /// shell is not installed here.
    fn run_in(shell: &str, script: &str) -> Option<String> {
        let no_rc: &[&str] = match shell {
            "bash" => &["--norc", "--noprofile"],
            "zsh" => &["-f"],
            _ => &["--no-config"],
        };
        let out = std::process::Command::new(shell).args(no_rc).arg("-c").arg(script).output();
        let Ok(out) = out else {
            eprintln!("{shell} is not installed; skipping");
            return None;
        };
        assert!(out.status.success(), "{shell} failed on {script:?}: {out:?}");
        Some(String::from_utf8(out.stdout).unwrap())
    }

    /// `shell` reads each of `words`, quoted, back as that same word.
    fn assert_reads_back(shell: &str, words: &[&str]) {
        let quoted: Vec<_> = words.iter().map(|word| quote(word)).collect();
        let script = format!("printf '<%s>' {}", quoted.join(" "));
        let expected: String = words.iter().map(|word| format!("<{word}>")).collect();
        if let Some(got) = run_in(shell, &script) {
            assert_eq!(got, expected, "{shell}: {script:?}");
        }
    }

    #[rstest]
    fn shells_read_quoted_words_back(#[values("bash", "zsh", "fish")] shell: &str) {
        assert_reads_back(shell, NASTY);
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]

        /// Any printable text, newlines and tabs included, survives quoting in every shell.
        #[rstest]
        fn any_word_survives_quoting(word in "[\\PC\n\t]{0,16}") {
            for shell in SHELLS {
                assert_reads_back(shell, &[&word]);
            }
        }
    }

    /// The whole rendered command, `cd` included, runs as intended in each shell.
    #[rstest]
    fn shells_run_a_rendered_plan(#[values("bash", "zsh", "fish")] shell: &str) {
        let root = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(root.path()).unwrap().join(r"-it's a \ dir プ $x");
        std::fs::create_dir(&dir).unwrap();
        let plan = ResumePlan {
            program: "printf".into(),
            args: vec!["%s|".into(), "it's".into(), r"a\b".into(), "$HOME".into()],
            cwd: Some(dir.clone()),
            cwd_requirement: CwdRequirement::Required,
            native_path: None,
        };
        let script = format!("{} && pwd", plan.render().unwrap());
        if let Some(got) = run_in(shell, &script) {
            assert_eq!(got, format!("it's|a\\b|$HOME|{}\n", dir.display()), "{shell}");
        }
    }

    #[rstest]
    fn render_without_a_cwd_is_the_command_alone() {
        let plan = plan(&ResumeTarget::new("s1"), CwdRequirement::Preferred, "codex", [
            "resume".into(),
            "s1".into(),
        ]);
        assert_eq!(plan.render().unwrap(), "codex resume s1");
    }

    #[rstest]
    fn render_puts_cd_first() {
        let target = ResumeTarget::new("s1").with_cwd("/w/my proj");
        let plan =
            plan(&target, CwdRequirement::Preferred, "claude", ["--resume".into(), "s1".into()]);
        assert_eq!(plan.render().unwrap(), "cd -- '/w/my proj' && claude --resume s1");
    }

    #[rstest]
    fn prepare_rejects_a_required_cwd_that_is_gone() {
        let target = ResumeTarget::new("s1").with_cwd("/no/such/dir");
        let err = plan(&target, CwdRequirement::Required, "pi", []).prepare().unwrap_err();
        assert_eq!(err, ResumeError::CwdMissing(Some("/no/such/dir".into())));
        let err = plan(&ResumeTarget::new("s1"), CwdRequirement::Required, "pi", []).prepare();
        assert_eq!(err.unwrap_err(), ResumeError::CwdMissing(None));
    }

    #[rstest]
    fn prepare_drops_a_preferred_cwd_that_is_gone() {
        let target = ResumeTarget::new("s1").with_cwd("/no/such/dir");
        let plan = plan(&target, CwdRequirement::Preferred, "claude", []).prepare().unwrap();
        assert_eq!(plan.cwd, None);
    }

    #[rstest]
    fn prepare_keeps_a_cwd_that_exists() {
        let dir = tempfile::tempdir().unwrap();
        let target = ResumeTarget::new("s1").with_cwd(dir.path());
        let plan = plan(&target, CwdRequirement::Required, "pi", []).prepare().unwrap();
        assert_eq!(plan.cwd.as_deref(), Some(dir.path()));
    }

    fn full_target() -> ResumeTarget {
        ResumeTarget::new("abc").with_cwd("/w/it's here").with_native_path("/s/x y.jsonl")
    }

    #[rstest]
    fn template_substitutes_each_placeholder_in_its_word() {
        let base = plan(&full_target(), CwdRequirement::Preferred, "claude", []);
        let plan = base
            .with_template(
                "my-agent --resume={id} 'literal word' {path} --dir {cwd}",
                &full_target(),
            )
            .unwrap();
        assert_eq!(plan.program, "my-agent");
        assert_eq!(plan.args, [
            "--resume=abc",
            "literal word",
            "/s/x y.jsonl",
            "--dir",
            "/w/it's here"
        ]);
        assert_eq!(plan.cwd_requirement, CwdRequirement::Preferred);
        assert_eq!(
            plan.render().unwrap(),
            r"cd -- '/w/it'\''s here' && my-agent --resume=abc 'literal word' '/s/x y.jsonl' --dir '/w/it'\''s here'"
        );
    }

    #[rstest]
    fn template_leaves_other_braces_alone() {
        let base = plan(&full_target(), CwdRequirement::None, "x", []);
        let plan = base.with_template("x {} {nope} {{id}}", &full_target()).unwrap();
        assert_eq!(plan.args, ["{}", "{nope}", "{abc}"]);
    }

    #[rstest]
    #[case::path("x {path}", ResumeError::TemplateValueMissing("path"))]
    #[case::cwd("x {cwd}", ResumeError::TemplateValueMissing("cwd"))]
    #[case::empty("  ", ResumeError::InvalidTemplate("the template is empty".into()))]
    #[case::unbalanced("x 'y", ResumeError::InvalidTemplate("unbalanced quotes".into()))]
    fn template_errors(#[case] template: &str, #[case] expected: ResumeError) {
        let target = ResumeTarget::new("abc");
        let base = plan(&target, CwdRequirement::None, "x", []);
        assert_eq!(base.with_template(template, &target).unwrap_err(), expected);
    }

    mod harnesses {
        use rstest::rstest;

        use super::super::*;
        use crate::harnesstools::{AnyHarness, Harness};

        fn harness(name: &str) -> AnyHarness {
            AnyHarness::from_name(name).unwrap()
        }

        const CWD: &str = "/home/u/it's a dir/プロジェクト";
        const QUOTED_CWD: &str = r"'/home/u/it'\''s a dir/プロジェクト'";

        #[rstest]
        #[case::claude(
            "claude",
            false,
            &["claude", "--resume", "0b3c-11"],
            CwdRequirement::Preferred,
        )]
        #[case::claude_fork(
            "claude",
            true,
            &["claude", "--resume", "0b3c-11", "--fork-session"],
            CwdRequirement::Preferred,
        )]
        #[case::codex("codex", false, &["codex", "resume", "0b3c-11"], CwdRequirement::Preferred)]
        #[case::codex_fork("codex", true, &["codex", "fork", "0b3c-11"], CwdRequirement::Preferred)]
        #[case::opencode(
            "opencode",
            false,
            &["opencode", "--session", "0b3c-11"],
            CwdRequirement::Preferred,
        )]
        #[case::opencode_fork(
            "opencode",
            true,
            &["opencode", "--session", "0b3c-11", "--fork"],
            CwdRequirement::Preferred,
        )]
        #[case::pi_by_id("pi", false, &["pi", "--session", "0b3c-11"], CwdRequirement::Required)]
        #[case::pi_fork_by_id("pi", true, &["pi", "--fork", "0b3c-11"], CwdRequirement::Required)]
        fn plans_by_id(
            #[case] name: &str,
            #[case] fork: bool,
            #[case] argv: &[&str],
            #[case] requirement: CwdRequirement,
        ) {
            let target = ResumeTarget::new("0b3c-11").with_cwd(CWD).forked(fork);
            let plan = harness(name).resume(&target, None).unwrap();
            assert_eq!(plan.program, argv[0]);
            assert_eq!(plan.args, &argv[1..]);
            assert_eq!(plan.cwd_requirement, requirement);
            assert_eq!(plan.cwd.as_deref(), Some(Path::new(CWD)));
            assert_eq!(plan.render().unwrap(), format!("cd -- {QUOTED_CWD} && {}", argv.join(" ")));
        }

        #[rstest]
        #[case::continued(false, "--session")]
        #[case::forked(true, "--fork")]
        fn pi_resumes_a_known_file_by_path(#[case] fork: bool, #[case] flag: &str) {
            let path =
                "/home/u/.pi/agent/sessions/--home-u-it's--/2026-09-18T10-00-00-000Z_0b3c.jsonl";
            let target =
                ResumeTarget::new("0b3c").with_cwd(CWD).with_native_path(path).forked(fork);
            let plan = harness("pi").resume(&target, None).unwrap();
            assert_eq!(plan.args, [flag, path]);
            assert_eq!(plan.cwd_requirement, CwdRequirement::Preferred);
            assert_eq!(plan.native_path.as_deref(), Some(Path::new(path)));
            assert_eq!(
                plan.render().unwrap(),
                format!(
                    r"cd -- {QUOTED_CWD} && pi {flag} '/home/u/.pi/agent/sessions/--home-u-it'\''s--/2026-09-18T10-00-00-000Z_0b3c.jsonl'"
                )
            );
        }

        #[rstest]
        fn codex_resumes_a_reverted_rollout_by_its_thread() {
            let id = "0a1b2c3d-4e5f-6789-abcd-ef0123456789_99999999-4e5f-6789-abcd-ef0123456789";
            let plan = harness("codex").resume(&ResumeTarget::new(id), None).unwrap();
            assert_eq!(plan.args, ["resume", "0a1b2c3d-4e5f-6789-abcd-ef0123456789"]);
        }

        #[rstest]
        fn a_claude_subagent_is_not_resumable() {
            let err = harness("claude").resume(&ResumeTarget::new("agent-a1b2"), None);
            assert!(matches!(err, Err(ResumeError::NotResumable(_))));
            // Not even through a template: the harness decides what can be resumed.
            let err = harness("claude").resume(&ResumeTarget::new("agent-a1b2"), Some("x {id}"));
            assert!(matches!(err, Err(ResumeError::NotResumable(_))));
        }

        #[rstest]
        fn a_template_keeps_the_harness_cwd_requirement() {
            let target = ResumeTarget::new("s 1").with_cwd(CWD);
            let plan = harness("pi").resume(&target, Some("my-pi --resume {id} -C {cwd}")).unwrap();
            assert_eq!(plan.program, "my-pi");
            assert_eq!(plan.args, ["--resume", "s 1", "-C", CWD]);
            assert_eq!(plan.cwd_requirement, CwdRequirement::Required);
            assert_eq!(
                plan.render().unwrap(),
                format!("cd -- {QUOTED_CWD} && my-pi --resume 's 1' -C {QUOTED_CWD}")
            );
        }

        #[cfg(unix)]
        #[rstest]
        fn a_non_utf8_path_is_an_error() {
            use std::os::unix::ffi::OsStrExt;
            let bad = PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/\xff.jsonl"));
            let target = ResumeTarget::new("s").with_native_path(&bad);
            let err = harness("pi").resume(&target, None).unwrap_err();
            assert_eq!(err, ResumeError::NonUtf8Path(bad.clone()));
            let plan = harness("codex").resume(&ResumeTarget::new("s").with_cwd(&bad), None);
            assert_eq!(plan.unwrap().render().unwrap_err(), ResumeError::NonUtf8Path(bad));
        }
    }

    mod locate {
        use std::fs;

        use rstest::{fixture, rstest};
        use tempfile::TempDir;

        use super::super::*;
        use crate::harnesstools::{ccode, codex, opencode, pi};

        /// A directory standing in for a harness's session root, removed on drop.
        #[fixture]
        fn root() -> TempDir {
            tempfile::tempdir().unwrap()
        }

        fn touch(path: &Path, body: &str) -> PathBuf {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
            path.to_path_buf()
        }

        #[rstest]
        fn claude_finds_a_transcript_in_any_project(root: TempDir) {
            let root = root.path();
            touch(&root.join("-work-a/other.jsonl"), "");
            let want = touch(&root.join("-work-b é/0b3c-11.jsonl"), "");
            // A subagent's transcript under a session is no session of its own name.
            touch(&root.join("-work-a/0b3c-11/subagents/agent-x.jsonl"), "");
            assert_eq!(ccode::session::locate(root, "0b3c-11"), Some(want));
            assert_eq!(ccode::session::locate(root, "missing"), None);
            assert_eq!(ccode::session::locate(&root.join("nope"), "0b3c-11"), None);
        }

        #[rstest]
        #[case::traversal("../../etc/passwd")]
        #[case::dot("..")]
        #[case::empty("")]
        #[case::separator("a/b")]
        fn an_id_that_is_no_plain_name_is_never_looked_up(root: TempDir, #[case] id: &str) {
            touch(&root.path().join("p/x.jsonl"), "");
            assert_eq!(ccode::session::locate(root.path(), id), None);
            assert_eq!(codex::session::locate(root.path(), id), None);
        }

        #[rstest]
        fn codex_finds_a_rollout_by_its_id(root: TempDir) {
            let root = root.path();
            let id = "0a1b2c3d-4e5f-6789-abcd-ef0123456789";
            touch(
                &root.join(
                    "2026/09/18/rollout-2026-09-18T00-00-00-ffffffff-4e5f-6789-abcd-ef0123456789.\
                     jsonl",
                ),
                "",
            );
            touch(&root.join(format!("2026/09/19/rollout-2026-09-19T00-00-00-{id}.jsonl")), "");
            // A later segment of the thread: the one Codex resumes it from.
            let segment = format!("{id}_99999999-4e5f-6789-abcd-ef0123456789");
            let want = touch(
                &root.join(format!("2026/09/20/rollout-2026-09-20T00-00-00-{segment}.jsonl")),
                "",
            );
            assert_eq!(codex::session::locate(root, id), Some(want.clone()));
            // A session captured as the segment before segments were the thread's.
            assert_eq!(codex::session::locate(root, &segment), Some(want));
            assert_eq!(codex::session::locate(root, "0a1b2c3d"), None);
        }

        fn pi_header(id: &str) -> String {
            format!(
                "{}\n",
                serde_json::json!({"type": "session", "version": 3, "id": id, "cwd": "/w"})
            )
        }

        #[rstest]
        fn pi_finds_a_session_by_its_file_name_and_header(root: TempDir) {
            let root = root.path();
            touch(&root.join("--w--/2026-09-18T10-00-00-000Z_other.jsonl"), &pi_header("other"));
            let want = touch(
                &root.join("--w x--/2026-09-18T11-00-00-000Z_0199.jsonl"),
                &pi_header("0199"),
            );
            assert_eq!(pi::session::locate(root, "0199"), Some(want));
            assert_eq!(pi::session::locate(root, "missing"), None);
        }

        #[rstest]
        fn pi_trusts_the_header_over_the_file_name(root: TempDir) {
            let root = root.path();
            // Named for `0199`, but its header says otherwise.
            touch(&root.join("--w--/1_0199.jsonl"), &pi_header("imposter"));
            let want = touch(&root.join("--w--/my notes.jsonl"), &pi_header("0199"));
            assert_eq!(pi::session::locate(root, "0199"), Some(want));
            // Neither is a pi session without a header.
            touch(&root.join("--w--/2_0200.jsonl"), "{\"type\":\"message\"}\n");
            assert_eq!(pi::session::locate(root, "0200"), None);
        }

        #[rstest]
        #[tokio::test]
        async fn opencode_finds_a_session_in_its_database(#[from(root)] dir: TempDir) {
            use sqlx::Connection;
            let db = dir.path().join("opencode.db");
            let opts =
                sqlx::sqlite::SqliteConnectOptions::new().filename(&db).create_if_missing(true);
            let mut conn = sqlx::SqliteConnection::connect_with(&opts).await.unwrap();
            crate::db::query::<sqlx::Sqlite>(
                "CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT)",
            )
            .execute(&mut conn)
            .await
            .unwrap();
            crate::db::query::<sqlx::Sqlite>("INSERT INTO session VALUES ('ses_1', '/w')")
                .execute(&mut conn)
                .await
                .unwrap();
            conn.close().await.unwrap();

            assert_eq!(opencode::session::locate(&db, "ses_1").await, Some(db.clone()));
            assert_eq!(opencode::session::locate(&db, "ses_2").await, None);
            assert_eq!(opencode::session::locate(&dir.path().join("none.db"), "ses_1").await, None);
        }

        #[rstest]
        #[tokio::test]
        async fn opencode_finds_a_2_0_session(#[from(root)] dir: TempDir) {
            use sqlx::Connection;
            let db = dir.path().join("opencode.db");
            let opts =
                sqlx::sqlite::SqliteConnectOptions::new().filename(&db).create_if_missing(true);
            let mut conn = sqlx::SqliteConnection::connect_with(&opts).await.unwrap();
            crate::db::query::<sqlx::Sqlite>("CREATE TABLE session_v2 (id TEXT PRIMARY KEY)")
                .execute(&mut conn)
                .await
                .unwrap();
            crate::db::query::<sqlx::Sqlite>("INSERT INTO session_v2 VALUES ('ses_2')")
                .execute(&mut conn)
                .await
                .unwrap();
            conn.close().await.unwrap();

            assert_eq!(opencode::session::locate(&db, "ses_2").await, Some(db.clone()));
        }
    }
}
