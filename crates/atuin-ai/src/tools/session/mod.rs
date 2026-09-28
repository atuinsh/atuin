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

use crate::commands::session::{harness_name, one_line};
use crate::tools::ToolOutcome;

async fn connect(settings: &Settings) -> Result<AiClient, ToolOutcome> {
    AiClient::from_settings(settings).await.map_err(|e| {
        ToolOutcome::Error(format!(
            "AI sessions are unavailable: could not connect to the Atuin daemon ({e}). Shell \
             history search still works."
        ))
    })
}

/// Every captured session (of `harness`, if given), newest first, decoded into domain types.
pub async fn list_sessions(
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
///
/// `None`, or a blank string (models often pass `""` for "no filter"), is no filter.
fn resolve_cwd(cwd: Option<&str>) -> Result<Option<PathBuf>, ToolOutcome> {
    let Some(cwd) = cwd.map(str::trim).filter(|c| !c.is_empty()) else {
        return Ok(None);
    };
    let path = crate::tools::expand_path(cwd);
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
    Ok(Some(normalise(&joined)))
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
        let Some(cwd) = s.cwd.as_deref().filter(|c| !c.starts_with(root)) else {
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

/// A Claude Code subagent: a fragment of the session that spawned it, only meaningful as part of
/// it. Not any session with a parent: forks, branches and continuations have one too, and those
/// are sessions a person ran. (Only Claude Code subagents are told apart today, by their
/// `agent-` transcript names; the parsers do not record what kind of link a parent is.)
fn is_subagent(s: &Session) -> bool {
    s.parent.is_some() && s.handle.session.as_ref().starts_with("agent-")
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
        let relation = if is_subagent(s) {
            "subagent of"
        } else {
            "continues from"
        };
        let _ = writeln!(out, "   {relation} {}", parent.session);
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
    use atuin_client::ai_session::{HarnessSession, NativeSessionId};
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::absolute("/a/b/../c", "/a/c")]
    #[case::trailing_dot("/a/./b", "/a/b")]
    #[case::whitespace("  /a/b  ", "/a/b")]
    fn resolve_cwd_normalises(#[case] input: &str, #[case] want: &str) {
        assert_eq!(resolve_cwd(Some(input)).ok().unwrap(), Some(PathBuf::from(want)));
    }

    #[rstest]
    fn resolve_cwd_expands_home() {
        let home = std::env::home_dir().unwrap();
        assert_eq!(resolve_cwd(Some("~")).ok().unwrap(), Some(home.clone()));
        assert_eq!(resolve_cwd(Some("~/src/x")).ok().unwrap(), Some(home.join("src/x")));
    }

    #[rstest]
    fn blank_cwd_is_no_filter() {
        assert_eq!(resolve_cwd(None).ok().unwrap(), None);
        assert_eq!(resolve_cwd(Some("  ")).ok().unwrap(), None);
    }

    #[rstest]
    fn only_claude_code_agent_children_are_subagents() {
        let parent = HarnessSession {
            harness: HarnessKind::ClaudeCode,
            session: NativeSessionId::from("p".to_owned()),
        };
        let with = |id: &str, parent: Option<HarnessSession>| {
            let mut s = fixtures::session(id, None, time::OffsetDateTime::UNIX_EPOCH);
            s.parent = parent;
            s
        };
        assert!(is_subagent(&with("agent-a1", Some(parent.clone()))));
        assert!(!is_subagent(&with("fork-uuid", Some(parent))), "a fork is a session of its own");
        assert!(!is_subagent(&with("agent-a1", None)));
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
