//! MCP tools over AI-agent sessions captured by the daemon: `atuin_ai_session_list`,
//! `atuin_ai_session_search`, and `atuin_ai_session_read`.

pub mod caller;
pub mod list;
pub mod read;
pub mod search;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use atuin_client::ai_session::{HarnessKind, Session};
use atuin_client::settings::Settings;
use atuin_common::time::OffsetDateTimeExt;
use atuin_daemon::AiClient;
use futures::{StreamExt, TryStreamExt};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::commands::session::{harness_name, one_line};
use crate::tools::ToolOutcome;

// Only harnesses a capture path can actually produce are offered as filters (see AnyHarness);
// Copilot has no capture source yet, so advertising it would return empty for every query.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessFilter {
    ClaudeCode,
    Codex,
    Opencode,
    Pi,
}

impl From<HarnessFilter> for HarnessKind {
    fn from(value: HarnessFilter) -> Self {
        match value {
            HarnessFilter::ClaudeCode => Self::ClaudeCode,
            HarnessFilter::Codex => Self::Codex,
            HarnessFilter::Opencode => Self::Opencode,
            HarnessFilter::Pi => Self::Pi,
        }
    }
}

async fn connect(settings: &Settings) -> Result<AiClient, ToolOutcome> {
    AiClient::from_settings(settings).await.map_err(|e| {
        ToolOutcome::Error(format!(
            "AI sessions are unavailable: could not connect to the Atuin daemon ({e}). Shell \
             history search still works."
        ))
    })
}

/// A `cwd` argument that says anything: models often pass `""` for "no filter".
fn cwd_filter(cwd: Option<&str>) -> Option<&str> {
    cwd.filter(|c| !c.trim().is_empty())
}

/// Every captured session (of `harness`, if given), newest first, decoded into domain types.
async fn list_sessions(
    client: &mut AiClient,
    harness: Option<HarnessKind>,
) -> Result<Vec<Session>, String> {
    client
        .list_sessions(harness)
        .await
        .map_err(|e| e.to_string())?
        .map(|s| {
            s.map_err(|e| e.to_string())
                .and_then(|s| Session::try_from(s).map_err(|e| e.to_string()))
        })
        .try_collect()
        .await
}

/// Resolve a `cwd` filter as the model wrote it: relative paths (including `.`) are taken from
/// the directory the MCP server was launched in, which Claude Code, Codex and opencode all set to
/// the project. A client that launches it from the home directory (or `/`) has not said where the
/// project is, and `.` would silently match nearly everything, so that is an error.
fn resolve_cwd(cwd: &str) -> Result<PathBuf, ToolOutcome> {
    let path = crate::tools::expand_path(cwd.trim());
    let joined = if path.is_absolute() {
        path
    } else {
        let home = std::env::home_dir();
        let dir = std::env::current_dir()
            .ok()
            .filter(|dir| dir.parent().is_some() && home.as_ref().is_none_or(|home| dir != home));
        let Some(dir) = dir else {
            return Err(ToolOutcome::Error(format!(
                "cwd {cwd:?} is relative, but this MCP server was not started in a project \
                 directory, so it cannot be resolved. Pass the project's absolute path."
            )));
        };
        dir.join(path)
    };
    Ok(normalise(&joined))
}

/// Normalise `.`/`..` lexically; the session may be from another machine, so the path need not
/// exist here and cannot be canonicalised.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// What a `cwd` filter left out: the directories of matching sessions outside `root`, busiest
/// first, with directories sharing the project's name first of all (the same repository checked
/// out elsewhere, often on another machine whose sessions arrived by sync). `None` when nothing
/// was left out. With `same_name_only` other directories are not mentioned; `partial` says
/// `others` was cut off, so the counts are lower bounds.
fn elsewhere_note<'a>(
    root: &Path,
    others: impl IntoIterator<Item = &'a Session>,
    same_name_only: bool,
    partial: bool,
) -> Option<String> {
    let name = root.file_name();
    let mut dirs: Vec<(&Path, usize, bool)> = Vec::new();
    for s in others {
        let Some(cwd) = s.cwd.as_deref().filter(|c| !is_under(c, root)) else {
            continue;
        };
        let same = name.is_some() && cwd.file_name() == name;
        if same_name_only && !same {
            continue;
        }
        match dirs.iter_mut().find(|(d, ..)| *d == cwd) {
            Some((_, n, _)) => *n += 1,
            None => dirs.push((cwd, 1, same)),
        }
    }
    if dirs.is_empty() {
        return None;
    }
    dirs.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)));
    let total: usize = dirs.iter().map(|(_, n, _)| n).sum();
    let shown = dirs
        .iter()
        .take(3)
        .map(|(dir, n, same)| {
            let hint = if *same {
                ", same project name"
            } else {
                ""
            };
            format!("{} ({n}{hint})", dir.display())
        })
        .collect::<Vec<_>>()
        .join(", ");
    let at_least = if partial {
        "at least "
    } else {
        ""
    };
    let more = if dirs.len() > 3 {
        ", …"
    } else {
        ""
    };
    Some(format!(
        "{at_least}{total} more outside {}: {shown}{more}. Pass one of those as cwd, or omit cwd, \
         to include them.",
        root.display()
    ))
}

fn is_under(cwd: &Path, root: &Path) -> bool {
    cwd.starts_with(root)
}

/// A session spawned by another one (a Claude Code subagent, say): only meaningful as part of
/// its parent.
fn is_subagent(s: &Session) -> bool {
    s.parent.is_some()
}

/// The session's title, falling back to its opening prompt.
fn label(s: &Session) -> String {
    s.title
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .or(s.preview.as_deref())
        .map_or_else(String::new, |text| one_line(text, 120))
}

fn timestamp(ts: time::OffsetDateTime, offset: time::UtcOffset) -> String {
    ts.checked_to_offset(offset)
        .map_or_else(|| "unknown time".to_owned(), |local| local.display().ymd_hm().to_string())
}

/// The compact per-session line shared by list and search results:
/// `[harness] <updated>  <id>  (<n> msgs)` then the directory/branch and title on their own lines.
fn render_session_summary(out: &mut String, index: usize, s: &Session, offset: time::UtcOffset) {
    let _ = writeln!(
        out,
        "{index}. [{}] {}  {}  ({} msgs)",
        harness_name(s.handle.harness),
        timestamp(s.updated_at, offset),
        s.handle.session,
        s.message_count,
    );
    let label = label(s);
    if !label.is_empty() {
        let _ = writeln!(out, "   {label}");
    }
    if let Some(cwd) = s.cwd.as_deref().map(Path::display) {
        let branch = s.git_branch.as_deref().filter(|b| !b.is_empty() && *b != "HEAD");
        let _ =
            writeln!(out, "   in {cwd}{}", branch.map(|b| format!(" ({b})")).unwrap_or_default());
    }
    if let Some(parent) = &s.parent {
        let _ = writeln!(out, "   subagent of {}", parent.session);
    }
    // How it ended, so a reader can tell which session holds the answer without opening each.
    if let Some(reply) = s.last_reply.as_deref().map(|r| one_line(r, 240)).filter(|r| !r.is_empty())
    {
        let _ = writeln!(out, "   last reply: {reply}");
    }
}

#[cfg(test)]
pub(super) mod fixtures {
    use atuin_client::ai_session::{HarnessKind, HarnessSession, NativeSessionId, Session};
    use atuin_common::harnesstools::session::Usage;

    pub fn session(id: &str, cwd: Option<&str>, started: time::OffsetDateTime) -> Session {
        Session::builder()
            .handle(HarnessSession {
                harness: HarnessKind::ClaudeCode,
                session: NativeSessionId::from(id.to_owned()),
            })
            .cwd(cwd.map(Into::into))
            .started_at(started)
            .updated_at(started)
            .usage(Usage::default())
            .build()
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::absolute("/a/b/../c", "/a/c")]
    #[case::trailing_dot("/a/./b", "/a/b")]
    #[case::whitespace("  /a/b  ", "/a/b")]
    fn resolve_cwd_normalises(#[case] input: &str, #[case] want: &str) {
        assert_eq!(resolve_cwd(input).ok().unwrap(), PathBuf::from(want));
    }

    #[rstest]
    fn resolve_cwd_expands_home() {
        let home = std::env::home_dir().unwrap();
        assert_eq!(resolve_cwd("~").ok().unwrap(), home);
        assert_eq!(resolve_cwd("~/src/x").ok().unwrap(), home.join("src/x"));
    }

    #[rstest]
    fn blank_cwd_is_no_filter() {
        assert_eq!(cwd_filter(Some("  ")), None);
        assert_eq!(cwd_filter(Some(".")), Some("."));
    }

    #[rstest]
    fn is_under_matches_whole_components() {
        let root = Path::new("/work/atuin");
        assert!(is_under(Path::new("/work/atuin"), root));
        assert!(is_under(Path::new("/work/atuin/crates"), root));
        assert!(!is_under(Path::new("/work/atuin.sh"), root));
    }

    #[rstest]
    fn elsewhere_note_puts_the_same_project_first() {
        let at = |cwd: &str| fixtures::session("s", Some(cwd), time::OffsetDateTime::UNIX_EPOCH);
        let others = [
            at("/work/atuin/crates"),
            at("/Users/e/scratch"),
            at("/Users/e/scratch"),
            at("/Users/e/workspace/atuin"),
        ];
        let root = Path::new("/work/atuin");
        let note = elsewhere_note(root, &others, false, false).unwrap();
        assert!(
            note.starts_with(
                "3 more outside /work/atuin: /Users/e/workspace/atuin (1, same project name), \
                 /Users/e/scratch (2)"
            ),
            "{note}"
        );
        let same = elsewhere_note(root, &others, true, false).unwrap();
        assert!(same.starts_with("1 more outside"), "{same}");
        assert!(elsewhere_note(root, &others[..1], false, false).is_none());
        let partial = elsewhere_note(root, &others, false, true).unwrap();
        assert!(partial.starts_with("at least 3 more outside"), "{partial}");
    }
}
