//! Grok Build: `<grok home>/sessions/<encoded cwd>/<session>/chat_history.jsonl` paired with
//! `summary.json`, `updates.jsonl`, and `.atuin.json`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{Value, json};

use super::liveness::{agent_running, changed_lately};
use super::{
    AppendOptions, AppendOutcome, Dirs, Links, Liveness, LocalTip, Processes, ReplaceOutcome, Seen,
    SessionSync, Stamp, SyncError, append_jsonl, blocking, blocking_liveness, branch_point,
    check_segment, continuing_from, jsonl_lines, kept_lines, modified, past, replace_jsonl,
    require_idle, resumes_from, switched_to,
};
use crate::harnesstools::grok::Grok;
use crate::harnesstools::grok::rehydrate::{self, Rendered, join_jsonl, timestamp};
use crate::harnesstools::grok::session::{
    AtuinSidecar, Chain, GrokMessage, GrokSummary, SIDECAR_FILE, SUMMARY_FILE, UPDATES_FILE,
    compacted_beside, default_root, locate, meta_id, read_summary_beside, read_update_times_beside,
    title_id,
};
use crate::harnesstools::rehydrate::RehydrateMessage;
use crate::harnesstools::session::{Content, Message as _, Role};

impl SessionSync for Grok {
    async fn local_tip(&self, id: &str) -> Result<Option<LocalTip>, SyncError> {
        let (root, id) = (default_root(), id.to_owned());
        blocking(move || match locate(&root, &id) {
            Some(path) => Ok(Some(Transcript::read(&path, &id)?.tip(&path))),
            None => Ok(None),
        })
        .await
    }

    async fn is_live(&self, id: &str, cwd: Option<&Path>) -> Liveness {
        let (root, id, cwd) = (default_root(), id.to_owned(), cwd.map(Path::to_path_buf));
        blocking_liveness(move || {
            let file = locate(&root, &id);
            let seen = Seen {
                dirs: Dirs::of(cwd),
                changing: file.as_deref().is_some_and(|f| changed_lately(modified(f))),
                names: vec![id],
            };
            liveness(&Processes::here(), &seen)
        })
        .await
    }

    async fn append(
        &self,
        id: &str,
        base: &LocalTip,
        lines: &[RehydrateMessage],
        options: &AppendOptions<'_>,
    ) -> Result<AppendOutcome, SyncError> {
        let seen = base.seen(options.cwd, vec![id.to_owned()]);
        require_idle(blocking_liveness(move || liveness(&Processes::here(), &seen)).await)?;
        let (id, base, lines) = (id.to_owned(), base.clone(), lines.to_vec());
        let taken = options.taken_ids.cloned();
        blocking(move || append_to(&id, &base, &lines, taken.as_ref())).await
    }

    async fn replace(
        &self,
        id: &str,
        base: &LocalTip,
        branch: &[RehydrateMessage],
        options: &AppendOptions<'_>,
        backups: &Path,
    ) -> Result<ReplaceOutcome, SyncError> {
        let seen = base.seen(options.cwd, vec![id.to_owned()]);
        require_idle(blocking_liveness(move || liveness(&Processes::here(), &seen)).await)?;
        let (id, base, branch, backups) =
            (id.to_owned(), base.clone(), branch.to_vec(), backups.to_path_buf());
        let taken = options.taken_ids.cloned();
        blocking(move || replace_to(&id, &base, &branch, taken.as_ref(), &backups)).await
    }
}

pub(super) fn liveness(procs: &Processes, seen: &Seen) -> Liveness {
    agent_running(procs, "grok", &["grok-build"], seen)
}

struct Transcript {
    session_id: String,
    bytes: Vec<u8>,
    modified: Option<std::time::SystemTime>,
    summary: GrokSummary,
    /// The mappings of the lines the transcript has.
    sidecar: AtuinSidecar,
    /// Every mapping `.atuin.json` holds, those of lines only a compaction's request still has
    /// included: what a write keeps.
    stored: AtuinSidecar,
    last_gk: Option<String>,
    prompt_count: u64,
    line_ids: Vec<String>,
    known: HashSet<String>,
}

impl Transcript {
    fn read(path: &Path, id: &str) -> Result<Self, SyncError> {
        let modified = modified(path);
        let bytes = std::fs::read(path)?;
        let summary = read_summary_beside(path);
        let sidecar = AtuinSidecar::read_beside(path);
        let mut file = Self::parse(bytes, modified, id, summary, sidecar);
        // What a compaction replaced is the session's still, read from its request.
        for lines in compacted_beside(path) {
            let mut chain = Chain::default();
            for line in lines {
                if let Ok(mut msg) = serde_json::from_value::<GrokMessage>(line)
                    && let Some(gk) = chain.key(&mut msg)
                {
                    file.known.insert(file.stored.ids.get(&gk).cloned().unwrap_or(gk));
                }
            }
        }
        // A compaction keeps no prompt's index in the transcript: the next one's is past theirs.
        let prompted = read_update_times_beside(path).prompts_by_index.into_keys().max();
        file.prompt_count = file.prompt_count.max(prompted.map_or(0, |index| index + 1));
        Ok(file)
    }

    fn parse(
        bytes: Vec<u8>,
        modified: Option<std::time::SystemTime>,
        id: &str,
        summary: GrokSummary,
        sidecar: AtuinSidecar,
    ) -> Self {
        let mut chain = Chain::default();
        let mut seen_gk = HashSet::new();
        let mut line_ids = Vec::new();
        let mut known = HashSet::new();
        let mut prompt_count = 0u64;

        if summary.present {
            known.insert(meta_id(id));
            if let Some(title) = &summary.title {
                known.insert(title_id(id, title));
            }
        }

        for line in jsonl_lines(&bytes) {
            let Ok(mut msg) = GrokMessage::decode(line) else {
                continue;
            };
            let Some(gk) = chain.key(&mut msg) else {
                continue;
            };
            // The next prompt's index: past every prompt's, and the one a compaction replays.
            if let Some(index) = msg.prompt_index() {
                prompt_count = prompt_count.max(index + 1);
            } else if msg.role() == crate::harnesstools::session::Role::User {
                prompt_count += 1;
            }
            seen_gk.insert(gk.clone());
            let resolved = sidecar.ids.get(&gk).cloned().unwrap_or(gk);
            known.insert(resolved.clone());
            line_ids.push(resolved);
        }

        let active_ids: HashMap<String, String> = sidecar
            .ids
            .iter()
            .filter(|(gk, _)| seen_gk.contains(*gk))
            .map(|(gk, id)| (gk.clone(), id.clone()))
            .collect();
        let mut active_merged = HashMap::new();
        for (dropped, into) in &sidecar.merged {
            if !known.contains(dropped) && known.contains(into) {
                known.insert(dropped.clone());
                active_merged.insert(dropped.clone(), into.clone());
            }
        }
        Self {
            session_id: id.to_owned(),
            bytes,
            modified,
            summary,
            sidecar: AtuinSidecar {
                ids: active_ids,
                merged: active_merged,
            },
            stored: sidecar,
            last_gk: chain.last().map(str::to_owned),
            prompt_count,
            line_ids,
            known,
        }
    }

    fn tip(self, path: &Path) -> LocalTip {
        self.local_tip(path)
    }

    fn local_tip(&self, path: &Path) -> LocalTip {
        let tip_source_id = self
            .line_ids
            .last()
            .cloned()
            .or_else(|| self.summary.present.then(|| meta_id(&self.session_id)));
        LocalTip {
            native_path: path.to_path_buf(),
            known_source_ids: self.known.clone(),
            merged: self.sidecar.merged.clone(),
            tip_source_id,
            stamp: Stamp::of(&self.bytes),
            modified: self.modified,
            cwd: self.summary.cwd.clone(),
            unswitchable: None,
        }
    }
}

fn extend(
    id: &str,
    file: &Transcript,
    base: &LocalTip,
    lines: &[RehydrateMessage],
    taken: Option<&HashSet<String>>,
) -> Result<Rendered, SyncError> {
    check_segment(lines, base, taken, Links::Chained)?;
    let mut prev_gk = file.last_gk.clone();
    let after = base.tip_source_id.as_deref();
    Ok(rehydrate::render_messages(id, lines, &mut prev_gk, file.prompt_count, after))
}

fn write_sidecar(transcript_path: &Path, sidecar: &AtuinSidecar) -> Result<(), SyncError> {
    let sidecar_path = transcript_path.with_file_name(SIDECAR_FILE);
    if sidecar.is_empty() {
        if sidecar_path.exists() {
            std::fs::remove_file(&sidecar_path)?;
        }
        return Ok(());
    }
    let bytes =
        serde_json::to_vec_pretty(sidecar).map_err(|err| SyncError::Other(err.to_string()))?;
    if sidecar_path.exists() {
        crate::fs::replace(&sidecar_path, &bytes, |_| true)
            .map_err(|err| SyncError::Other(err.to_string()))?;
    } else {
        crate::fs::write_new(&sidecar_path, &bytes)?;
    }
    Ok(())
}

/// What `updates.jsonl` held before [`write_updates`], and what it wrote there: what
/// [`restore_updates`] puts back, and what it checks is still there first.
struct StagedUpdates {
    before: Option<Vec<u8>>,
    after: Vec<u8>,
}

/// Write `update_lines`, the scrollback of what a catch-up or switch writes, to `updates.jsonl`
/// beside `transcript_path`: after what it holds, or (`replace`) in its place. A catch-up that
/// stopped before its transcript took the lines left their scrollback there already, and it is
/// not written again.
fn write_updates(
    transcript_path: &Path,
    update_lines: &[Value],
    replace: bool,
) -> Result<StagedUpdates, SyncError> {
    let path = transcript_path.with_file_name(UPDATES_FILE);
    let before = std::fs::read(&path).ok();
    let body = join_jsonl(update_lines).into_bytes();
    let after = match &before {
        Some(_) if replace => {
            crate::fs::replace(&path, &body, |_| true)
                .map_err(|err| SyncError::Other(err.to_string()))?;
            body
        }
        Some(held) if !body.is_empty() && !held.ends_with(&body) => {
            append_jsonl(&path, Stamp::of(held), update_lines)?;
            let torn = !held.is_empty() && !held.ends_with(b"\n");
            [
                &held[..],
                if torn {
                    b"\n"
                } else {
                    b""
                },
                &body,
            ]
            .concat()
        }
        None if !body.is_empty() => {
            crate::fs::write_new(&path, &body)?;
            body
        }
        _ => before.clone().unwrap_or_default(),
    };
    Ok(StagedUpdates { before, after })
}

/// Put `updates.jsonl` beside `transcript_path` back as it was before [`write_updates`], unless
/// Grok Build has written to it since.
fn restore_updates(transcript_path: &Path, staged: StagedUpdates) {
    let path = transcript_path.with_file_name(UPDATES_FILE);
    match staged.before {
        Some(bytes) => {
            let _ = crate::fs::replace(&path, &bytes, |now| now == staged.after);
        }
        None if std::fs::read(&path).is_ok_and(|now| now == staged.after) => {
            let _ = std::fs::remove_file(&path);
        }
        None => {}
    }
}

/// Bring `summary.json` beside `transcript_path` up to the rows a catch-up or switch wrote.
fn touch_summary(transcript_path: &Path, appended_rows: &[RehydrateMessage]) {
    let summary_path = transcript_path.with_file_name(SUMMARY_FILE);
    let Ok(bytes) = std::fs::read(&summary_path) else {
        return;
    };
    let Ok(mut summary) = serde_json::from_slice::<Value>(&bytes) else {
        return;
    };
    if let Some(latest) = appended_rows.iter().map(|r| r.timestamp).max() {
        let ts = timestamp(latest);
        summary["updated_at"] = json!(ts);
        summary["last_active_at"] = json!(ts);
    }
    if let Ok(transcript_bytes) = std::fs::read(transcript_path) {
        let count = jsonl_lines(&transcript_bytes).count();
        summary["num_chat_messages"] = json!(count);
    }
    if let Ok(updated) = serde_json::to_vec_pretty(&summary) {
        let _ = crate::fs::replace(&summary_path, &updated, |_| true);
    }
}

pub(super) fn append_to(
    id: &str,
    base: &LocalTip,
    lines: &[RehydrateMessage],
    taken: Option<&HashSet<String>>,
) -> Result<AppendOutcome, SyncError> {
    let path = &base.native_path;
    let _lock = rehydrate::lock(path.parent().unwrap_or(path))?;
    let file = Transcript::read(path, id)?;
    if Stamp::of(&file.bytes) != base.stamp {
        return Err(SyncError::Changed);
    }
    let rendered = extend(id, &file, base, lines, taken)?;
    if !rendered.chat_lines.is_empty() {
        let prior_sidecar = file.stored.clone();
        let mut combined_sidecar = file.stored;
        combined_sidecar.ids.extend(rendered.sidecar.ids);
        combined_sidecar.merged.extend(rendered.sidecar.merged);
        // Write `.atuin.json` BEFORE appending to `chat_history.jsonl` so capture never sees new
        // lines before their ID mappings exist on disk; roll back `.atuin.json` if `append_jsonl`
        // detects a concurrent edit.
        write_sidecar(path, &combined_sidecar)?;
        let updates = match write_updates(path, &rendered.update_lines, false) {
            Ok(updates) => updates,
            Err(err) => {
                let _ = write_sidecar(path, &prior_sidecar);
                return Err(err);
            }
        };
        if let Err(err) = append_jsonl(path, base.stamp, &rendered.chat_lines) {
            restore_updates(path, updates);
            let _ = write_sidecar(path, &prior_sidecar);
            return Err(err);
        }
        touch_summary(path, lines);
    }
    Ok(AppendOutcome {
        native_path: path.clone(),
        tip_source_id: rendered
            .written_source_ids
            .last()
            .cloned()
            .or_else(|| base.tip_source_id.clone()),
        appended: rendered.written_source_ids,
    })
}

pub(super) fn replace_to(
    id: &str,
    base: &LocalTip,
    branch: &[RehydrateMessage],
    taken: Option<&HashSet<String>>,
    backups: &Path,
) -> Result<ReplaceOutcome, SyncError> {
    let path = &base.native_path;
    if branch.is_empty() {
        return Err(SyncError::Unsupported("the branch has no rows"));
    }
    let _lock = rehydrate::lock(path.parent().unwrap_or(path))?;
    let file = Transcript::read(path, id)?;
    if Stamp::of(&file.bytes) != base.stamp {
        return Err(SyncError::Changed);
    }
    let (shared, from) = branch_point(base, branch, Links::Chained)?;
    let kept_bytes = shared_lines(&file.bytes, &file.sidecar, id, &from);
    let mut kept = Transcript::parse(
        kept_bytes,
        file.modified,
        id,
        file.summary.clone(),
        file.sidecar.clone(),
    );
    let continuing = continuing_from(kept.local_tip(path), &from)?;
    let rest = past(branch, shared, &continuing, Links::Chained);
    // The scrollback numbers the branch's prompts from its first, and so do the lines written
    // past those kept: a compaction leaves the kept ones no number to go on from.
    let first = rest
        .first()
        .and_then(|r| branch.iter().position(|m| m.source_id == r.source_id))
        .unwrap_or(branch.len());
    let prompts = branch[..first].iter().filter(|m| {
        m.role == Role::User
            && m.content
                .iter()
                .any(|c| matches!(c, Content::Text(t) | Content::Summary(t) if !t.is_empty()))
    });
    kept.prompt_count = u64::try_from(prompts.count()).unwrap_or(u64::MAX);
    let rendered = extend(id, &kept, &continuing, &rest, taken)?;

    let mut out = kept.bytes;
    for line in &rendered.chat_lines {
        out.extend(line.to_string().bytes());
        out.push(b'\n');
    }

    let prior_sidecar = file.stored.clone();
    let mut combined_sidecar = kept.sidecar;
    combined_sidecar.ids.extend(rendered.sidecar.ids);
    combined_sidecar.merged.extend(rendered.sidecar.merged);

    let written = Transcript::parse(
        out.clone(),
        file.modified,
        id,
        file.summary.clone(),
        combined_sidecar.clone(),
    )
    .tip(path);
    resumes_from(written.tip_source_id.as_deref(), &written.merged, switched_to(&rest, &from))?;

    // `.atuin.json` gains the branch's mappings before `chat_history.jsonl` is replaced, and keeps
    // the old ones: a mapping only ever names the line it is the key of, and what a compaction
    // replaced is read through them again.
    let mut staged_sidecar = prior_sidecar.clone();
    staged_sidecar.ids.extend(combined_sidecar.ids.clone());
    staged_sidecar.merged.extend(combined_sidecar.merged.clone());
    write_sidecar(path, &staged_sidecar)?;
    let mut full_prev_gk = None;
    let full_updates =
        rehydrate::render_messages(id, branch, &mut full_prev_gk, 0, None).update_lines;
    let updates = match write_updates(path, &full_updates, true) {
        Ok(updates) => updates,
        Err(err) => {
            let _ = write_sidecar(path, &prior_sidecar);
            return Err(err);
        }
    };

    let (backup, warning) = match replace_jsonl(path, base.stamp, &file.bytes, &out, backups) {
        Ok(res) => res,
        Err(err) => {
            restore_updates(path, updates);
            let _ = write_sidecar(path, &prior_sidecar);
            return Err(err);
        }
    };
    touch_summary(path, branch);

    Ok(ReplaceOutcome {
        native_path: path.clone(),
        appended: rendered.written_source_ids,
        tip_source_id: written.tip_source_id,
        backup,
        warning,
    })
}

/// Keep every line up to and including the line whose resolved capture key is `from` (preserving
/// any leading `system` prompt line Grok Build wrote). When `from` is `atuin-meta-<id>`, the two
/// branches share only `summary.json` metadata and no conversational lines, so only a leading
/// `system` prompt line is kept.
fn shared_lines(bytes: &[u8], sidecar: &AtuinSidecar, session_id: &str, from: &str) -> Vec<u8> {
    let mut chain = Chain::default();
    let mut past_branch = from == meta_id(session_id);
    kept_lines(bytes, |raw| {
        let Ok(mut msg) = GrokMessage::decode(raw) else {
            return None;
        };
        let gk = chain.key(&mut msg)?;
        if past_branch {
            return Some(false);
        }
        let resolved = sidecar.ids.get(&gk).map_or(gk.as_str(), String::as_str);
        if resolved == from {
            past_branch = true;
        }
        Some(true)
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rstest::rstest;
    use time::OffsetDateTime;

    use super::*;
    use crate::harnesstools::rehydrate::RehydrateSession;

    const ID: &str = "01a18221-a7b8-75e0-b6ef-b2a5cc95d2c9";

    fn row(id: &str, parent: Option<&str>, role: Role, text: &str, at: i64) -> RehydrateMessage {
        RehydrateMessage {
            source_id: id.to_owned(),
            parent_source_id: parent.map(str::to_owned),
            timestamp: OffsetDateTime::from_unix_timestamp(at).unwrap(),
            role,
            content: vec![Content::Text(text.to_owned())],
            model: None,
            provider: None,
            usage: None,
            stop_reason: None,
            turn_id: None,
            cwd: None,
            git_branch: None,
        }
    }

    fn seed_session(root: &Path, home: &Path, messages: Vec<RehydrateMessage>) -> PathBuf {
        let initial = RehydrateSession {
            id: ID.to_owned(),
            title: Some("Sync test".to_owned()),
            cwd: PathBuf::from("/work/proj"),
            original_cwd: None,
            git_branch: None,
            model: None,
            started_at: OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
            messages,
            fork_of: None,
        };
        rehydrate::rehydrate_into(root, home, &initial).unwrap()
    }

    #[rstest]
    fn append_and_replace_fast_forward_and_switch_branches() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sessions");
        let backups = dir.path().join("backups");
        let path = seed_session(&root, dir.path(), vec![
            row("u1", None, Role::User, "first", 1_700_000_001),
            row("a1", Some("u1"), Role::Assistant, "reply 1", 1_700_000_002),
        ]);
        let tip = Transcript::read(&path, ID).unwrap().tip(&path);
        assert_eq!(tip.tip_source_id.as_deref(), Some("a1"));

        let added = [
            row("u2", Some("a1"), Role::User, "local follow-up", 1_700_000_003),
            row("a2", Some("u2"), Role::Assistant, "local reply", 1_700_000_004),
        ];
        let outcome = append_to(ID, &tip, &added, None).unwrap();
        assert_eq!(outcome.appended, ["u2", "a2"]);
        assert_eq!(outcome.tip_source_id.as_deref(), Some("a2"));

        let diverged_tip = Transcript::read(&path, ID).unwrap().tip(&path);
        let other_branch = vec![
            row("u1", None, Role::User, "first", 1_700_000_001),
            row("a1", Some("u1"), Role::Assistant, "reply 1", 1_700_000_002),
            row("u3", Some("a1"), Role::User, "remote follow-up", 1_700_000_005),
            row("a3", Some("u3"), Role::Assistant, "remote reply", 1_700_000_006),
        ];
        let replaced = replace_to(ID, &diverged_tip, &other_branch, None, &backups).unwrap();
        assert_eq!(replaced.appended, ["u3", "a3"]);
        assert_eq!(replaced.tip_source_id.as_deref(), Some("a3"));
        assert!(replaced.backup.is_file());
    }

    #[rstest]
    fn switching_from_shared_session_meta_drops_entire_old_conversation_keeping_system_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sessions");
        let backups = dir.path().join("backups");
        let path = seed_session(&root, dir.path(), vec![
            row("u_old", None, Role::User, "old first prompt", 1_700_000_001),
            row("a_old", Some("u_old"), Role::Assistant, "old reply", 1_700_000_002),
        ]);
        // Prepend a Grok system prompt line to ensure it stays preserved while old turns drop.
        let existing = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            format!("{}\n{existing}", json!({"type": "system", "content": "Grok system prompt"})),
        )
        .unwrap();

        let tip = Transcript::read(&path, ID).unwrap().tip(&path);
        let meta = meta_id(ID);
        let new_branch = vec![
            RehydrateMessage {
                content: Vec::new(),
                ..row(&meta, None, Role::System, "", 1_700_000_000)
            },
            row("u_new", Some(&meta), Role::User, "brand new prompt", 1_700_000_010),
            row("a_new", Some("u_new"), Role::Assistant, "brand new reply", 1_700_000_011),
        ];
        let replaced = replace_to(ID, &tip, &new_branch, None, &backups).unwrap();
        assert_eq!(replaced.appended, ["u_new", "a_new"]);
        assert_eq!(replaced.tip_source_id.as_deref(), Some("a_new"));

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("Grok system prompt"));
        assert!(!after.contains("old first prompt"));
        assert!(after.contains("brand new prompt"));
    }

    #[rstest]
    fn append_survives_grok_rewriting_chat_history_with_system_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sessions");
        let path = seed_session(&root, dir.path(), vec![
            row("u1", None, Role::User, "first", 1_700_000_001),
            row("a1", Some("u1"), Role::Assistant, "reply 1", 1_700_000_002),
        ]);

        let existing = std::fs::read_to_string(&path).unwrap();
        let rewritten =
            format!("{}\n{existing}", json!({"type": "system", "content": "Grok system prompt"}));
        std::fs::write(&path, rewritten).unwrap();

        let tip = Transcript::read(&path, ID).unwrap().tip(&path);
        assert!(tip.known_source_ids.contains("u1"));
        assert!(tip.known_source_ids.contains("a1"));
        assert_eq!(tip.tip_source_id.as_deref(), Some("a1"));

        let added = [row("u2", Some("a1"), Role::User, "remote turn", 1_700_000_010)];
        let outcome = append_to(ID, &tip, &added, None).unwrap();
        assert_eq!(outcome.appended, ["u2"]);
    }

    #[rstest]
    fn append_and_replace_reject_concurrent_edits_taken_ids_and_disjoint_branches() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sessions");
        let backups = dir.path().join("backups");
        let path = seed_session(&root, dir.path(), vec![
            row("u1", None, Role::User, "first", 1_700_000_001),
            row("a1", Some("u1"), Role::Assistant, "reply 1", 1_700_000_002),
        ]);
        let stale_tip = Transcript::read(&path, ID).unwrap().tip(&path);

        let taken = HashSet::from(["u2".to_owned()]);
        let err = append_to(
            ID,
            &stale_tip,
            &[row("u2", Some("a1"), Role::User, "clash", 1_700_000_003)],
            Some(&taken),
        )
        .unwrap_err();
        assert!(matches!(err, SyncError::IdTaken(_)));

        let disjoint = vec![row("other", None, Role::User, "unrelated", 1_700_000_005)];
        let err = replace_to(ID, &stale_tip, &disjoint, None, &backups).unwrap_err();
        assert!(matches!(err, SyncError::Unsupported(_)));

        let err = replace_to(ID, &stale_tip, &[], None, &backups).unwrap_err();
        assert!(matches!(err, SyncError::Unsupported(_)));

        std::fs::write(&path, format!("{}\n", std::fs::read_to_string(&path).unwrap())).unwrap();
        let err = append_to(
            ID,
            &stale_tip,
            &[row("u2", Some("a1"), Role::User, "next", 1_700_000_003)],
            None,
        )
        .unwrap_err();
        assert!(matches!(err, SyncError::Changed));
    }

    #[rstest]
    fn switching_inside_a_merged_run_merges_into_the_new_branch() {
        use crate::harnesstools::session::{ToolCallId, ToolResult, ToolUse};

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sessions");
        let backups = dir.path().join("backups");
        let path = seed_session(&root, dir.path(), vec![
            row("u1", None, Role::User, "first", 1_700_000_001),
            RehydrateMessage {
                content: vec![Content::ToolUse(ToolUse {
                    id: ToolCallId::from("c1".to_owned()),
                    name: "run_terminal_command".to_owned(),
                    input: Value::Null,
                })],
                ..row("a1", Some("u1"), Role::Assistant, "", 1_700_000_002)
            },
            RehydrateMessage {
                content: vec![Content::ToolResult(ToolResult {
                    call: ToolCallId::from("c1".to_owned()),
                    output: Value::Null,
                    error: false,
                })],
                ..row("t1", Some("a1"), Role::Tool, "", 1_700_000_003)
            },
            row("a2", Some("t1"), Role::Assistant, "branch A done", 1_700_000_004),
        ]);

        let tip = Transcript::read(&path, ID).unwrap().tip(&path);
        assert_eq!(tip.merged.get("a1").map(String::as_str), Some("a2"));

        let branch_b = vec![
            row("u1", None, Role::User, "first", 1_700_000_001),
            RehydrateMessage {
                content: vec![Content::ToolUse(ToolUse {
                    id: ToolCallId::from("c1".to_owned()),
                    name: "run_terminal_command".to_owned(),
                    input: Value::Null,
                })],
                ..row("a1", Some("u1"), Role::Assistant, "", 1_700_000_002)
            },
            row("a_alt", Some("a1"), Role::Assistant, "branch B done", 1_700_000_005),
        ];
        let replaced = replace_to(ID, &tip, &branch_b, None, &backups).unwrap();
        assert_eq!(replaced.tip_source_id.as_deref(), Some("a_alt"));

        let sidecar = AtuinSidecar::read_beside(&path);
        assert_eq!(sidecar.merged.get("a1").map(String::as_str), Some("a_alt"));
    }

    /// A compaction moves the lines atuin wrote into its request; catching up keeps their ids,
    /// so the session read again from its start makes no rows twice.
    #[rstest]
    fn appending_after_a_compaction_keeps_the_ids_of_what_it_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sessions");
        let path = seed_session(&root, dir.path(), vec![
            row("u1", None, Role::User, "first", 1_700_000_001),
            row("a1", Some("u1"), Role::Assistant, "reply", 1_700_000_002),
        ]);
        let written: Vec<Value> = jsonl_lines(&std::fs::read(&path).unwrap())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        let requests = path.with_file_name("compaction_requests");
        std::fs::create_dir_all(&requests).unwrap();
        std::fs::write(
            requests.join("r1.json"),
            json!({"created_at": "2026-10-28T10:05:00Z", "chat_history": written}).to_string(),
        )
        .unwrap();
        let compacted = [
            json!({"type": "user", "content": [{"type": "text", "text": "<user_query>\nfirst\n</user_query>"}]}),
            json!({"type": "user", "content": [{"type": "text", "text": "This session is being continued."}], "synthetic_reason": "compaction_meta"}),
        ];
        std::fs::write(&path, join_jsonl(&compacted)).unwrap();

        let tip = Transcript::read(&path, ID).unwrap().tip(&path);
        let next = row("u2", tip.tip_source_id.as_deref(), Role::User, "next", 1_700_000_003);
        append_to(ID, &tip, &[next], None).unwrap();

        let ids = AtuinSidecar::read_beside(&path).ids;
        for id in ["u1", "a1", "u2"] {
            assert!(ids.values().any(|v| v == id), "{id} lost from {ids:?}");
        }
    }

    /// A switch past a compaction both branches hold numbers the prompts it writes as the
    /// scrollback it writes does, from the branch's first: capture times a prompt by its number.
    #[rstest]
    fn a_switch_past_a_compaction_numbers_its_prompts_as_its_scrollback() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sessions");
        let history = vec![
            row("u1", None, Role::User, "first", 1_700_000_001),
            row("a1", Some("u1"), Role::Assistant, "reply", 1_700_000_002),
            row("u2", Some("a1"), Role::User, "second", 1_700_000_003),
            row("a2", Some("u2"), Role::Assistant, "reply two", 1_700_000_004),
        ];
        let path = seed_session(&root, dir.path(), history.clone());
        let written: Vec<Value> = jsonl_lines(&std::fs::read(&path).unwrap())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        let requests = path.with_file_name("compaction_requests");
        std::fs::create_dir_all(&requests).unwrap();
        std::fs::write(
            requests.join("r1.json"),
            json!({"created_at": "2026-10-28T10:05:00Z", "chat_history": written}).to_string(),
        )
        .unwrap();
        let compacted = [
            json!({"type": "user", "content": [{"type": "text", "text": "<user_query>\nsecond\n</user_query>"}]}),
            json!({"type": "user", "content": [{"type": "text", "text": "This session is being continued."}], "synthetic_reason": "compaction_meta"}),
        ];
        std::fs::write(&path, join_jsonl(&compacted)).unwrap();
        let file = Transcript::read(&path, ID).unwrap();
        let (replayed, summary) = (file.line_ids[0].clone(), file.line_ids[1].clone());
        let tip = file.tip(&path);
        let local = row("u3", Some(&summary), Role::User, "here", 1_700_000_005);
        append_to(ID, &tip, &[local], None).unwrap();

        let mut branch = history;
        branch.push(RehydrateMessage {
            role: Role::System,
            ..row(&replayed, Some("a2"), Role::System, "second", 1_700_000_004)
        });
        branch.push(RehydrateMessage {
            content: vec![Content::Summary("This session is being continued.".to_owned())],
            ..row(&summary, Some(&replayed), Role::System, "", 1_700_000_004)
        });
        branch.push(row("r3", Some(&summary), Role::User, "there", 1_700_000_006));
        let tip = Transcript::read(&path, ID).unwrap().tip(&path);
        replace_to(ID, &tip, &branch, None, &dir.path().join("backups")).unwrap();

        let prompt_of =
            |line: &Value| line["content"][0]["text"].as_str().unwrap_or("").contains("there");
        let lines: Vec<Value> = jsonl_lines(&std::fs::read(&path).unwrap())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        let written = lines.iter().find(|line| prompt_of(line)).unwrap()["prompt_index"].clone();
        let updates = std::fs::read(path.with_file_name(UPDATES_FILE)).unwrap();
        let scrollback = jsonl_lines(&updates)
            .map(|line| serde_json::from_slice::<Value>(line).unwrap())
            .map(|line| line["params"]["update"].clone())
            .find(|update| update["content"]["text"].as_str().is_some_and(|t| t.contains("there")))
            .unwrap()["_meta"]["promptIndex"]
            .clone();
        assert_eq!((written, scrollback), (json!(2), json!(2)));
    }

    /// A line with nothing to key it by goes with the lines around it: a switch past it keeps it.
    #[rstest]
    fn a_switch_keeps_an_unkeyed_line_of_the_shared_history() {
        let lines = [
            json!({"type": "system", "content": "You are Grok."}),
            json!({"type": "user", "content": [{"type": "text", "text": "<user_query>\np1\n</user_query>"}], "prompt_index": 0}),
            json!({"type": "assistant", "content": ""}),
            json!({"type": "user", "content": [{"type": "text", "text": "<user_query>\np2\n</user_query>"}], "prompt_index": 1}),
            json!({"type": "assistant", "content": "after"}),
        ];
        let bytes = join_jsonl(&lines).into_bytes();
        let mut chain = Chain::default();
        let keys: Vec<_> = lines
            .iter()
            .filter_map(|l| chain.key(&mut serde_json::from_value(l.clone()).unwrap()))
            .collect();
        let kept = shared_lines(&bytes, &AtuinSidecar::default(), ID, &keys[1]);
        assert_eq!(kept, join_jsonl(&lines[..4]).into_bytes());
    }

    /// A row a restore writes no line for (context the daemon keeps without its text) is merged
    /// into the line before it, the session's metadata at the start: the copy holds every row,
    /// so a switch finds where a branch leaves it.
    #[rstest]
    fn a_restored_copy_holds_every_row() {
        let dir = tempfile::tempdir().unwrap();
        let context = |id: &str, parent: Option<&str>, at: i64| RehydrateMessage {
            content: Vec::new(),
            ..row(id, parent, Role::System, "", at)
        };
        let rows = vec![
            context("s1", None, 1_700_000_001),
            row("u1", Some("s1"), Role::User, "first", 1_700_000_002),
            context("s2", Some("u1"), 1_700_000_003),
            row("a1", Some("s2"), Role::Assistant, "reply", 1_700_000_004),
        ];
        let path = seed_session(&dir.path().join("sessions"), dir.path(), rows);
        let tip = Transcript::read(&path, ID).unwrap().tip(&path);
        for id in ["s1", "u1", "s2", "a1"] {
            assert!(tip.known_source_ids.contains(id), "{id} not held: {tip:?}");
        }
        assert_eq!(tip.merged.get("s1").map(String::as_str), Some(meta_id(ID).as_str()));
        assert_eq!(tip.merged.get("s2").map(String::as_str), Some("u1"));
    }

    /// A copy compacted after the branches parted holds what the compaction replaced too, read
    /// from its request: a switch finds where the other branch leaves it.
    #[rstest]
    fn a_compacted_copy_holds_what_the_compaction_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = seed_session(&dir.path().join("sessions"), dir.path(), vec![
            row("u1", None, Role::User, "first", 1_700_000_001),
            row("a1", Some("u1"), Role::Assistant, "reply", 1_700_000_002),
        ]);
        let written: Vec<Value> = jsonl_lines(&std::fs::read(&path).unwrap())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        let requests = path.with_file_name("compaction_requests");
        std::fs::create_dir_all(&requests).unwrap();
        std::fs::write(
            requests.join("r1.json"),
            json!({"created_at": "2026-10-28T10:05:00Z", "chat_history": written}).to_string(),
        )
        .unwrap();
        std::fs::write(&path, join_jsonl(&[json!({"type": "user", "content": [{"type": "text", "text": "This session is being continued."}], "synthetic_reason": "compaction_meta"})])).unwrap();

        let tip = Transcript::read(&path, ID).unwrap().tip(&path);
        for id in ["u1", "a1"] {
            assert!(tip.known_source_ids.contains(id), "{id} not held: {tip:?}");
        }
    }

    /// The scrollback a catch-up writes is written before the transcript: when it can't be, the
    /// catch-up fails, leaving the transcript as it was to try again.
    #[rstest]
    fn a_catch_up_that_cannot_write_its_scrollback_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = seed_session(&dir.path().join("sessions"), dir.path(), vec![row(
            "u1",
            None,
            Role::User,
            "first",
            1_700_000_001,
        )]);
        let updates = path.with_file_name(UPDATES_FILE);
        std::fs::remove_file(&updates).unwrap();
        std::fs::create_dir(&updates).unwrap();
        let before = std::fs::read(&path).unwrap();

        let tip = Transcript::read(&path, ID).unwrap().tip(&path);
        let next = row("a1", Some("u1"), Role::Assistant, "reply", 1_700_000_002);
        assert!(append_to(ID, &tip, &[next], None).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    /// Catch-ups of one session at once take turns: one writes it, the rest find it changed and
    /// leave what it wrote, its ids among it.
    #[rstest]
    fn catch_ups_of_one_session_at_once_keep_the_ids_one_wrote() {
        // The race is lost only now and then: run it often enough to lose it.
        for _ in 0..10 {
            let dir = tempfile::tempdir().unwrap();
            let path = seed_session(&dir.path().join("sessions"), dir.path(), vec![row(
                "u1",
                None,
                Role::User,
                "first",
                1_700_000_001,
            )]);
            let tip = Transcript::read(&path, ID).unwrap().tip(&path);
            let next = row("a1", Some("u1"), Role::Assistant, "reply", 1_700_000_002);
            let results: Vec<_> = std::thread::scope(|scope| {
                // Every catch-up starts before any is waited for.
                let mut catch_ups = Vec::new();
                for _ in 0..8 {
                    catch_ups.push(
                        scope.spawn(|| append_to(ID, &tip, std::slice::from_ref(&next), None)),
                    );
                }
                catch_ups.into_iter().map(|catch_up| catch_up.join().unwrap()).collect()
            });
            assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1, "{results:?}");
            let ids = AtuinSidecar::read_beside(&path).ids;
            assert!(ids.values().any(|v| v == "a1"), "a1 lost from {ids:?}");
        }
    }

    /// A catch-up that stopped between writing its scrollback and the transcript, tried again,
    /// writes the scrollback once.
    #[rstest]
    fn a_catch_up_tried_again_writes_its_scrollback_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = seed_session(&dir.path().join("sessions"), dir.path(), vec![row(
            "u1",
            None,
            Role::User,
            "first",
            1_700_000_001,
        )]);
        let updates = path.with_file_name(UPDATES_FILE);
        let before = std::fs::read(&path).unwrap();
        let next = row("a1", Some("u1"), Role::Assistant, "reply", 1_700_000_002);
        let tip = Transcript::read(&path, ID).unwrap().tip(&path);
        append_to(ID, &tip, std::slice::from_ref(&next), None).unwrap();
        let scrollback = std::fs::read(&updates).unwrap();

        // Stopped before the transcript took the reply: its scrollback and ids are written.
        std::fs::write(&path, &before).unwrap();
        let tip = Transcript::read(&path, ID).unwrap().tip(&path);
        append_to(ID, &tip, &[next], None).unwrap();
        assert_eq!(std::fs::read(&updates).unwrap(), scrollback);
    }

    /// A catch-up whose transcript can't take its lines takes back the scrollback it wrote, and
    /// only that: not what Grok Build wrote since.
    #[rstest]
    fn taking_back_scrollback_keeps_what_grok_wrote_since() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat_history.jsonl");
        let updates = path.with_file_name(UPDATES_FILE);
        std::fs::write(&updates, "{\"old\":1}\n").unwrap();

        let staged = write_updates(&path, &[json!({"ours": 1})], false).unwrap();
        restore_updates(&path, staged);
        assert_eq!(std::fs::read_to_string(&updates).unwrap(), "{\"old\":1}\n");

        let staged = write_updates(&path, &[json!({"ours": 1})], false).unwrap();
        let mut file = std::fs::OpenOptions::new().append(true).open(&updates).unwrap();
        std::io::Write::write_all(&mut file, b"{\"grok\":1}\n").unwrap();
        restore_updates(&path, staged);
        assert_eq!(
            std::fs::read_to_string(&updates).unwrap(),
            "{\"old\":1}\n{\"ours\":1}\n{\"grok\":1}\n"
        );
    }
}
