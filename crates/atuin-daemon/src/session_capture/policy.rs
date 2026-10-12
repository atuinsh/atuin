//! What capture keeps of a session beyond [`ToolCapture`](super::ToolCapture): the user's history
//! and output filters, applied to the commands an agent runs as to the ones they run themselves;
//! files whose contents are credentials; and redaction, with the user's own patterns.

use std::borrow::Cow;
use std::path::Path;

use atuin_client::settings::Settings;
use atuin_client::settings::output::CommandFilter;
use atuin_common::harnesstools::access::Access;
use atuin_common::harnesstools::session::ToolUse;
use atuin_common::secrets::files::SensitiveFiles;
use atuin_common::secrets::{self, REDACT_BUDGET, Redactor};
use regex::RegexSet;

use super::tools;

/// What a call is kept with, from the most kept to the least.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Verdict {
    /// Its input and output, as [`ToolCapture`](super::ToolCapture) keeps them.
    Keep,
    /// Its input, but not its output: a command whose output may hold a credential (`atuin key`,
    /// `gh auth token`, one `[output] command_filter` matches), or a read of a credential file.
    WithholdOutput,
    /// Its input but for what it writes, and not its output: a write to a credential file.
    WithholdWritten,
    /// Neither: a command history would not keep (`history_filter`, `secrets_filter`), or one run
    /// in a directory `cwd_filter` matches.
    Drop,
}

/// The settings capture applies to what it keeps.
#[derive(Clone, Debug)]
pub struct CapturePolicy {
    history_filter: RegexSet,
    cwd_filter: RegexSet,
    secrets_filter: bool,
    command_filter: CommandFilter,
    files: SensitiveFiles,
    redactor: Redactor,
    /// `redactor` with the user's own patterns alone.
    users: Redactor,
}

impl Default for CapturePolicy {
    /// The built-in rules alone.
    fn default() -> Self {
        Self {
            history_filter: RegexSet::empty(),
            cwd_filter: RegexSet::empty(),
            secrets_filter: true,
            command_filter: CommandFilter::default(),
            files: SensitiveFiles::default(),
            redactor: Redactor::default(),
            users: Redactor::new(Vec::new(), false),
        }
    }
}

impl CapturePolicy {
    #[must_use]
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            history_filter: settings.history_filter.clone(),
            cwd_filter: settings.cwd_filter.clone(),
            secrets_filter: settings.secrets_filter,
            command_filter: settings.output.command_filter().clone(),
            files: settings.security.sensitive_files(&settings.key_path),
            redactor: settings.security.redactor(true),
            users: settings.security.redactor(false),
        }
    }

    /// Whether history ignores what runs in `cwd`: then none of a session's calls there are kept.
    pub(super) fn ignores_dir(&self, cwd: &Path) -> bool {
        self.cwd_filter.is_match(&cwd.to_string_lossy())
    }

    /// What `call`, made in `cwd`, is kept with: the least any of what it touches is kept with (a
    /// Codex script runs several tools at once, see [`Access::all`]). A script whose calls
    /// weren't all read can't be judged: it is dropped, what it ran and wrote with it. (The
    /// commands it runs still reach shell history, filtered, through Codex's hooks.)
    pub(super) fn judge(&self, call: &ToolUse, cwd: Option<&Path>) -> Verdict {
        let (accesses, whole) = Access::all(&call.name, &call.input);
        if !whole {
            return Verdict::Drop;
        }
        let verdict = accesses
            .iter()
            .map(|access| self.judge_access(access, cwd))
            .max()
            .unwrap_or(Verdict::Keep);
        if verdict == Verdict::Keep && tools::runs_output_unsafe(&call.input) {
            return Verdict::WithholdOutput;
        }
        verdict
    }

    fn judge_access(&self, access: &Access, cwd: Option<&Path>) -> Verdict {
        let dir = access.workdir.as_deref().map(Path::new).or(cwd);
        if let Some(command) = &access.command {
            let ignored = self.history_filter.is_match(command)
                || (self.secrets_filter && secrets::contains_secret(command))
                || access.workdir.as_deref().is_some_and(|dir| self.ignores_dir(Path::new(dir)));
            if ignored {
                return Verdict::Drop;
            }
        }
        if access.writes.iter().any(|path| self.files.matches(path, dir)) {
            return Verdict::WithholdWritten;
        }
        let unsafe_command = access.command.as_deref().is_some_and(|command| {
            secrets::output_unsafe(command)
                || self.command_filter.is_match(command)
                || self.files.named_in_command(command, dir)
        });
        if unsafe_command || access.reads.iter().any(|path| self.files.matches(path, dir)) {
            return Verdict::WithholdOutput;
        }
        Verdict::Keep
    }

    /// Whether `path`, as a call made in `cwd` names it, is a file whose contents are credentials.
    pub(super) fn protects(&self, path: &str, cwd: Option<&Path>) -> bool {
        self.files.matches(path, cwd)
    }

    #[cfg(test)]
    pub(super) fn with_redact_patterns(patterns: Vec<regex::Regex>) -> Self {
        Self {
            redactor: Redactor::new(patterns.clone(), true),
            users: Redactor::new(patterns, false),
            ..Self::default()
        }
    }

    /// `text` redacted, or `None` when that takes too long (see [`REDACT_BUDGET`]): then it is
    /// better not kept.
    pub(super) fn redact<'a>(&self, text: &'a str) -> Option<Cow<'a, str>> {
        self.redactor.redact_within(text, REDACT_BUDGET)
    }

    /// [`Self::redact`] with the user's own patterns alone, which, unlike the built-in ones, can
    /// match across lines, any length.
    pub(super) fn redact_users<'a>(&self, text: &'a str) -> Option<Cow<'a, str>> {
        self.users.redact_within(text, REDACT_BUDGET)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::path::PathBuf;

    use atuin_common::harnesstools::session::ToolCallId;
    use rstest::{fixture, rstest};
    use serde_json::{Value, json};

    use super::*;

    #[fixture]
    pub(crate) fn policy() -> CapturePolicy {
        CapturePolicy {
            history_filter: RegexSet::new(["^psql"]).unwrap(),
            cwd_filter: RegexSet::new(["^/secret"]).unwrap(),
            secrets_filter: true,
            command_filter: CommandFilter::from(RegexSet::new(["^cat notes"]).unwrap()),
            files: SensitiveFiles::new(
                &["*.secret".to_owned()],
                &[],
                Some(PathBuf::from("/home/me")),
            ),
            redactor: Redactor::default(),
            users: Redactor::new(Vec::new(), false),
        }
    }

    fn call(name: &str, input: Value) -> ToolUse {
        ToolUse {
            id: ToolCallId::from("c".to_owned()),
            name: name.to_owned(),
            input,
        }
    }

    #[rstest]
    #[case::history_filter("Bash", json!({"command": "psql -c 'select 1'"}), Verdict::Drop)]
    #[case::secrets_filter("Bash", json!({"command": "atuin login -u me -p hunter2"}), Verdict::Drop)]
    #[case::cwd_filter("shell", json!({"command": ["ls"], "workdir": "/secret/x"}), Verdict::Drop)]
    #[case::command_filter("Bash", json!({"command": "cat notes.md"}), Verdict::WithholdOutput)]
    #[case::credential_command("Bash", json!({"command": "gh auth token"}), Verdict::WithholdOutput)]
    #[case::atuin_key("exec_command", json!(r#"{"cmd":"atuin key"}"#), Verdict::WithholdOutput)]
    #[case::cat_dotenv("Bash", json!({"command": "cat .env"}), Verdict::WithholdOutput)]
    #[case::read_dotenv("Read", json!({"file_path": "/w/.env"}), Verdict::WithholdOutput)]
    #[case::read_users_file("read", json!({"filePath": "notes.secret"}), Verdict::WithholdOutput)]
    #[case::read_key("Read", json!({"file_path": "~/.ssh/id_ed25519"}), Verdict::WithholdOutput)]
    #[case::grep_credentials("Grep", json!({"pattern": "x", "path": "~/.aws/credentials"}), Verdict::WithholdOutput)]
    #[case::write_dotenv("Write", json!({"file_path": ".env", "content": "A=1"}), Verdict::WithholdWritten)]
    #[case::patch_dotenv("apply_patch", json!("*** Begin Patch\n*** Update File: .env\n+A=1\n*** End Patch"), Verdict::WithholdWritten)]
    #[case::shell_write("Bash", json!({"command": "printf '%s' 'violetmeadow' > .env"}), Verdict::WithholdWritten)]
    #[case::opencode_patch("patch", json!({"patchText": "*** Begin Patch\n*** Update File: .env\n*** End Patch"}), Verdict::WithholdWritten)]
    #[case::read_source("Read", json!({"file_path": "src/main.rs"}), Verdict::Keep)]
    #[case::read_example("Read", json!({"file_path": ".env.example"}), Verdict::Keep)]
    #[case::ordinary("Bash", json!({"command": "cargo test"}), Verdict::Keep)]
    #[case::search("WebSearch", json!({"query": "rust"}), Verdict::Keep)]
    // Codex 0.160 runs each tool through a script; what it runs is judged as if run directly.
    #[case::script_history_filter("exec", json!("const r = await tools.exec_command({cmd:\"psql -c 'select 1'\",workdir:\"/w\"});\ntext(r.output);\n"), Verdict::Drop)]
    #[case::script_command_filter("exec", json!("const r = await tools.exec_command({cmd:\"cat notes.md\", max_output_tokens:200});\ntext(r.output);\n"), Verdict::WithholdOutput)]
    #[case::script_cwd_filter("exec", json!("await tools.exec_command({cmd:\"ls\",workdir:\"/secret/x\"})"), Verdict::Drop)]
    #[case::script_secret("exec", json!("await tools.exec_command({cmd:\"atuin login -u me -p hunter2\"})"), Verdict::Drop)]
    #[case::script_cat_dotenv("exec", json!("await tools.exec_command({cmd:'cat .env'})"), Verdict::WithholdOutput)]
    #[case::script_patch_dotenv("exec", json!("const patch = \"*** Begin Patch\\n*** Add File: .env\\n+A=1\\n*** End Patch\";\nconst r = await tools.apply_patch(patch);\ntext(r);\n"), Verdict::WithholdWritten)]
    #[case::script_read_key("exec", json!("await tools.view_image({path:\"~/.ssh/id_ed25519\"})"), Verdict::WithholdOutput)]
    #[case::script_worst_call("exec", json!("await tools.exec_command({cmd:'cat .env'});\nawait tools.exec_command({cmd:'psql'});"), Verdict::Drop)]
    #[case::script_computed("exec", json!("const c = `psql ${db}`;\nawait tools.exec_command({cmd: c});"), Verdict::Drop)]
    // A script not read whole is dropped, whatever it may hide, filtered or not (from review and
    // validation).
    #[case::script_misread_regex("exec", json!("if (ok) /\"/.test(x); await tools.exec_command({cmd:\"psql -c 1\"}); const y = \"z\";"), Verdict::Drop)]
    #[case::script_misread_division("exec", json!("let i = 0; i++ / 2; await tools.exec_command({cmd:\"psql\"});"), Verdict::Drop)]
    #[case::script_joined_list("exec", json!("await tools.exec_command({cmd: [\"psql\", \"-c 1\"].join(\" \")});"), Verdict::Drop)]
    #[case::script_appended("exec", json!("let c = \"ps\"; c += \"ql -c 1\"; await tools.exec_command({cmd: c});"), Verdict::Drop)]
    #[case::script_patch_built("exec", json!("const name = (await tools.exec_command({cmd: \"ls\"})).output.trim();\nawait tools.apply_patch(`*** Begin Patch\\n*** Update File: ${name}\\n+X=3\\n*** End Patch`);"), Verdict::Drop)]
    #[case::script_split_joined("exec", json!("await tools.exec_command({cmd: \"echo,hi\".split(\",\").join(\" \")});"), Verdict::Drop)]
    #[case::script_computed_workdir("exec", json!("await tools.exec_command({cmd: \"ls\", workdir: base + \"/x\"});"), Verdict::Drop)]
    #[case::script_ordinary("exec", json!("const r = await tools.exec_command({cmd:\"cargo test\"});\ntext(r.output);"), Verdict::Keep)]
    fn what_a_call_is_kept_with(
        policy: CapturePolicy,
        #[case] name: &str,
        #[case] input: Value,
        #[case] verdict: Verdict,
    ) {
        assert_eq!(policy.judge(&call(name, input), Some(Path::new("/w"))), verdict);
    }

    #[rstest]
    #[case::ignored("/secret/project", true)]
    #[case::other("/w", false)]
    fn directories_history_ignores(
        policy: CapturePolicy,
        #[case] cwd: &str,
        #[case] ignored: bool,
    ) {
        assert_eq!(policy.ignores_dir(Path::new(cwd)), ignored);
    }
}
