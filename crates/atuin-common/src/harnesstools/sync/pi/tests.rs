use std::path::PathBuf;

use pretty_assertions::assert_eq;
use rstest::rstest;
use serde_json::json;
use tempfile::TempDir;
use time::OffsetDateTime;

use super::*;
use crate::harnesstools::session::{Content, Role};
use crate::harnesstools::sync::TableEntry;
use crate::harnesstools::sync::liveness::tests::{self as live, Fake, dir, procs, table};

fn header(version: u64) -> Value {
    json!({"type": "session", "version": version, "id": "5e55e55e-0000-4000-8000-000000000002",
           "timestamp": "2026-09-28T10:00:00.000Z", "cwd": "/work/proj"})
}

fn entry(id: &str, parent: Option<&str>, role: &str) -> Value {
    json!({"type": "message", "id": id, "parentId": parent, "timestamp": "2026-09-28T10:00:01.000Z",
           "message": {"role": role, "content": [{"type": "text", "text": id}], "timestamp": 1}})
}

/// A pi file of `version`: a prompt and its answer.
fn local(version: u64) -> Vec<u8> {
    let mut out = Vec::new();
    for line in [
        header(version),
        entry("aaaa0001", None, "user"),
        entry("aaaa0002", Some("aaaa0001"), "assistant"),
    ] {
        out.extend(line.to_string().bytes());
        out.push(b'\n');
    }
    out
}

fn row(id: &str, parent: &str, role: Role) -> RehydrateMessage {
    RehydrateMessage {
        source_id: id.to_owned(),
        parent_source_id: Some(parent.to_owned()),
        timestamp: OffsetDateTime::from_unix_timestamp(1_790_000_000).unwrap(),
        role,
        content: vec![Content::Text(format!("said {id}"))],
        model: Some("m".to_owned()),
        usage: None,
        stop_reason: None,
        turn_id: None,
        cwd: None,
        git_branch: None,
    }
}

fn ahead() -> Vec<RehydrateMessage> {
    vec![row("bbbb0001", "aaaa0002", Role::User), row("bbbb0002", "bbbb0001", Role::Assistant)]
}

fn write(dir: &TempDir, bytes: &[u8]) -> PathBuf {
    let path = dir.path().join("2026-09-28T10-00-00-000Z_5e55e55e.jsonl");
    std::fs::write(&path, bytes).unwrap();
    path
}

/// The copy works where its header says.
#[rstest]
fn the_tip_records_where_the_copy_works(dir: TempDir) {
    let path = write(&dir, &local(3));
    assert_eq!(File::read(&path).unwrap().tip(&path).cwd, Some(PathBuf::from("/work/proj")));
}

/// Appended entries hang from the last one, and pi continues from the last of them.
#[rstest]
fn a_fast_forward_resumes_from_the_last_entry(dir: TempDir) {
    let path = write(&dir, &local(3));
    let base = File::read(&path).unwrap().tip(&path);
    assert_eq!(base.tip_source_id.as_deref(), Some("aaaa0002"));

    let outcome = append_to(&base, &ahead(), None).unwrap();

    assert_eq!(outcome.appended, ["bbbb0001", "bbbb0002"]);
    assert_eq!(outcome.tip_source_id.as_deref(), Some("bbbb0002"));
    let after = File::read(&path).unwrap().tip(&path);
    assert_eq!(after.tip_source_id.as_deref(), Some("bbbb0002"));
    for id in ["aaaa0001", "aaaa0002", "bbbb0001", "bbbb0002"] {
        assert!(after.known_source_ids.contains(id), "{id} reads back");
    }
    let bytes = std::fs::read(&path).unwrap();
    let first: Value = serde_json::from_slice(jsonl_lines(&bytes).nth(3).unwrap()).unwrap();
    assert_eq!(first["parentId"], "aaaa0002");
}

/// The ids capture gives the lines of session `id`, kept in `path`, reading them as it does.
async fn captured(id: &str, path: &Path) -> HashSet<String> {
    use futures::TryStreamExt;

    use crate::harnesstools::pi::session::PiSession;
    use crate::harnesstools::session::Session;

    let session = SessionId::from(id.to_owned());
    let reader = PiSession::open(
        session.clone(),
        path.to_path_buf(),
        crate::sync::BlockingPool::new(std::num::NonZeroUsize::MIN),
    );
    let messages: Vec<PiMessage> = reader.read().try_collect().await.unwrap();
    let mut occurrences = HashMap::new();
    messages.iter().filter_map(|m| capture_key(&session, m, &mut occurrences)).collect()
}

/// A fork's header names its parent by file, and capture names the parent by the id that file's
/// own header carries, whatever the file is called: a header keyed on its content (one with no
/// id of its own to key on) is keyed with that id here too, so the synced fork holds every line.
/// A parent that can't be read is named by its file name, by both.
#[rstest]
#[case::named_by_pi("2026-09-28T09-00-00-000Z_0199aaaa-bbbb.jsonl", true)]
#[case::custom_name("custom-name.jsonl", true)]
#[case::custom_name_relative("custom-name.jsonl", false)]
#[case::unreadable("missing.jsonl", true)]
#[tokio::test]
async fn a_forks_header_is_keyed_as_capture_keys_it(
    dir: TempDir,
    #[case] parent_name: &str,
    #[case] absolute: bool,
) {
    let parent = dir.path().join(parent_name);
    if parent_name != "missing.jsonl" {
        let header = json!({"type": "session", "version": 3, "id": "0199aaaa-bbbb",
                            "timestamp": "2026-09-28T09:00:00.000Z", "cwd": "/work/proj"});
        std::fs::write(&parent, format!("{header}\n")).unwrap();
    }
    let named = if absolute {
        parent.to_string_lossy().into_owned()
    } else {
        parent_name.into()
    };
    // Its `id` after its `timestamp`, as pi's v1 migration orders a line: keyed on its content.
    let header = json!({"type": "session", "version": 3, "timestamp": "2026-09-28T10:00:00.000Z",
                        "id": "5e55e55e-0000-4000-8000-000000000002", "cwd": "/work/proj",
                        "parentSession": named});
    let mut bytes = format!("{header}\n").into_bytes();
    bytes.extend_from_slice(&local(3)[jsonl_lines(&local(3)).next().unwrap().len() + 1..]);
    let path = write(&dir, &bytes);

    let tip = File::read(&path).unwrap().tip(&path);
    let captured = captured("5e55e55e-0000-4000-8000-000000000002", &path).await;

    assert!(tip.known_source_ids.iter().any(|id| id.starts_with("syn-")), "{tip:?}");
    assert_eq!(tip.known_source_ids, captured);
    assert!(tip.tip_source_id.is_some_and(|tip| captured.contains(&tip)));
}

#[rstest]
#[case::off_an_older_entry(3, vec![row("bbbb0001", "aaaa0001", Role::User)], "Unsupported")]
#[case::an_old_file(1, ahead(), "Unsupported")]
#[case::an_id_the_file_holds(3, vec![row("aaaa0001", "aaaa0002", Role::User)], "IdTaken")]
#[case::changed(3, ahead(), "Changed")]
fn what_is_no_clean_fast_forward_is_refused(
    dir: TempDir,
    #[case] version: u64,
    #[case] rows: Vec<RehydrateMessage>,
    #[case] expected: &str,
) {
    let path = write(&dir, &local(version));
    let base = File::read(&path).unwrap().tip(&path);
    if expected == "Changed" {
        std::fs::write(&path, local(2)).unwrap();
    }
    let before = std::fs::read(&path).unwrap();
    let err = append_to(&base, &rows, None).unwrap_err();
    assert!(format!("{err:?}").starts_with(expected), "{err:?}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[rstest]
#[case::a_pi_running(b"pi\0".as_slice(), Liveness::Unknown)]
#[case::none(b"bash\0".as_slice(), Liveness::NotLive)]
fn any_pi_running_may_be_writing(dir: TempDir, #[case] cmdline: &[u8], #[case] expected: Liveness) {
    let procs = procs(dir.path(), &[Fake::running(9, cmdline, "1")]);
    assert_eq!(liveness(&procs, &Seen::default()), expected);
}

/// pi hides its command line behind its name: which session a pi works on is told only by where
/// it works.
#[rstest]
#[case::in_the_sessions_directory("proj", Liveness::Unknown)]
#[case::elsewhere("other", Liveness::NotLive)]
fn a_pi_is_told_apart_by_where_it_works(
    dir: TempDir,
    #[case] works_in: &str,
    #[case] expected: Liveness,
) {
    let procs = table(&[TableEntry {
        cwd: Some(dir.path().join(works_in)),
        ..live::entry(9, "pi", &["pi"], 1)
    }]);
    assert_eq!(liveness(&procs, &live::cwd(Some(dir.path().join("proj")))), expected);
}

/// A pi working elsewhere may have the session open from there (`pi --session <file>`): it counts
/// when its command line names the session's file, or while the file was modified lately.
#[rstest]
#[case::changed_lately_named_nowhere(10, false, Liveness::Unknown)]
#[case::changed_long_ago_named_nowhere(3600, false, Liveness::NotLive)]
#[case::changed_long_ago_named(3600, true, Liveness::Unknown)]
fn a_pi_elsewhere_counts_when_named_or_writing(
    dir: TempDir,
    #[case] ago: u64,
    #[case] named: bool,
    #[case] expected: Liveness,
) {
    let path = write(&dir, &local(2));
    let at = std::time::SystemTime::now() - std::time::Duration::from_secs(ago);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(at).unwrap();
    let mut base = File::read(&path).unwrap().tip(&path);
    assert_eq!(base.modified, Some(at));
    // Where the header records it working, as a directory still there.
    let proj = dir.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    base.cwd = Some(proj.clone());
    let file = path.display().to_string();
    let cmd: &[&str] = if named {
        &["pi", "--session", &file]
    } else {
        &["pi"]
    };
    let procs = table(&[TableEntry {
        cwd: Some(dir.path().join("other")),
        ..live::entry(9, "pi", cmd, 1)
    }]);
    let seen = appending(&base, "5e55e55e-0000-4000-8000-000000000002", Some(&proj));
    assert_eq!(liveness(&procs, &seen), expected);
}

/// A session file that went its own way here after sharing a turn with a tool call in it:
/// aaaa0001 → aaaa0002 (calling a tool, with the arguments capture doesn't keep) → aaaa0010
/// (its result, with an image capture doesn't keep), named "mine"; then, here only, aaaa0003,
/// and the session renamed after it.
fn went_its_own_way() -> Vec<Value> {
    let mut call = entry("aaaa0002", Some("aaaa0001"), "assistant");
    call["message"]["content"] = json!([
        {"type": "text", "text": "aaaa0002"},
        {"type": "toolCall", "id": "call_1", "name": "bash", "arguments": {"command": "ls -la ~"}},
    ]);
    let mut result = entry("aaaa0010", Some("aaaa0002"), "toolResult");
    result["message"]["toolCallId"] = json!("call_1");
    result["message"]["content"] = json!([
        {"type": "text", "text": "only this machine saw this output"},
        {"type": "image", "mimeType": "image/png", "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB"},
    ]);
    let name = |id: &str, parent: &str, name: &str| {
        json!({"type": "session_info", "id": id, "parentId": parent, "name": name,
               "timestamp": "2026-09-28T10:00:02.000Z"})
    };
    vec![
        header(3),
        entry("aaaa0001", None, "user"),
        call,
        result,
        name("cccc0001", "aaaa0010", "mine"),
        entry("aaaa0003", Some("cccc0001"), "user"),
        name("cccc0002", "aaaa0003", "renamed here"),
    ]
}

fn jsonl(lines: &[Value]) -> Vec<u8> {
    let mut out = Vec::new();
    for line in lines {
        out.extend(line.to_string().bytes());
        out.push(b'\n');
    }
    out
}

/// Another machine's branch off aaaa0010, as synced.
fn their_branch() -> Vec<RehydrateMessage> {
    let mut first = row("aaaa0001", "", Role::User);
    first.parent_source_id = None;
    vec![
        first,
        row("aaaa0002", "aaaa0001", Role::Assistant),
        row("aaaa0010", "aaaa0002", Role::Tool),
        row("bbbb0001", "aaaa0010", Role::User),
        row("bbbb0002", "bbbb0001", Role::Assistant),
    ]
}

/// Switching the copy to another machine's branch keeps its header, the entries it shares with
/// the branch byte for byte (the tool call's arguments and the image, which capture never kept),
/// and the session's names, even the one given after it went its own way, as pi takes the last
/// for the session's name; drops the entry it went on with here; and appends the branch's
/// entries past where they part, the first hanging from there, so pi resumes from the head. In
/// place, with the file as it was kept whole outside pi's sessions directory.
#[rstest]
fn a_switch_keeps_the_shared_entries_and_the_names(dir: TempDir) {
    let backups = tempfile::tempdir().unwrap();
    let local = jsonl(&went_its_own_way());
    let path = write(&dir, &local);
    let base = File::read(&path).unwrap().tip(&path);

    let outcome = replace_to(&base, &their_branch(), None, backups.path()).unwrap();

    assert_eq!(outcome.native_path, path);
    assert_eq!(outcome.appended, ["bbbb0001", "bbbb0002"]);
    assert_eq!(outcome.tip_source_id.as_deref(), Some("bbbb0002"));
    let after = File::read(&path).unwrap();
    let lines: Vec<&[u8]> = jsonl_lines(&after.bytes).collect();
    let was: Vec<&[u8]> = jsonl_lines(&local).collect();
    assert_eq!(lines[..5], was[..5], "the header, shared entries and name, byte for byte");
    assert_eq!(lines[5], was[6], "the name given here");
    let ids: Vec<&str> = after.entries.iter().map(|(id, ..)| id.as_str()).collect();
    let want = ["aaaa0001", "aaaa0002", "aaaa0010", "cccc0001", "cccc0002", "bbbb0001", "bbbb0002"];
    assert_eq!(ids, want);
    let parents: HashMap<&str, Option<&str>> =
        after.entries.iter().map(|(id, parent, _)| (id.as_str(), parent.as_deref())).collect();
    assert_eq!(parents["bbbb0001"], Some("aaaa0010"));
    assert_eq!(after.tip(&path).tip_source_id.as_deref(), Some("bbbb0002"));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no temporary file left");
    assert_eq!(outcome.backup.parent(), Some(backups.path()));
    assert_eq!(std::fs::read(&outcome.backup).unwrap(), local);
}

/// Nothing is written, or backed up, to a file changed since it was read, a version 1 file (pi
/// gives its entries new ids), or one sharing nothing with the branch.
#[rstest]
#[case::changed(3, true, their_branch(), "Changed")]
#[case::version_1(1, false, their_branch(), "Unsupported")]
#[case::shares_nothing(3, false, vec![row("dddd0001", "", Role::User)], "Unsupported")]
fn a_switch_that_cant_be_made_writes_nothing(
    dir: TempDir,
    #[case] version: u64,
    #[case] changed: bool,
    #[case] branch: Vec<RehydrateMessage>,
    #[case] expected: &str,
) {
    let backups = tempfile::tempdir().unwrap();
    let mut lines = went_its_own_way();
    lines[0] = header(version);
    let path = write(&dir, &jsonl(&lines));
    let base = File::read(&path).unwrap().tip(&path);
    if changed {
        lines.push(entry("aaaa0009", Some("aaaa0003"), "user"));
    }
    let now = jsonl(&lines);
    std::fs::write(&path, &now).unwrap();
    let err = replace_to(&base, &branch, None, backups.path()).unwrap_err();
    assert!(format!("{err:?}").starts_with(expected), "{err:?}");
    assert_eq!(std::fs::read(&path).unwrap(), now);
    assert_eq!(std::fs::read_dir(backups.path()).unwrap().count(), 0, "no backup left");
}

/// [`entry`] as pi's conversion of a version 1 file leaves it: `id` and `parentId` given after
/// the entry's other fields (session-manager.ts `migrateV1ToV2`).
fn migrated(id: &str, parent: Option<&str>, role: &str) -> Value {
    let mut fields = entry(id, parent, role).as_object().unwrap().clone();
    let id = fields.shift_remove("id").unwrap();
    let parent = fields.shift_remove("parentId").unwrap();
    fields.insert("id".to_owned(), id);
    fields.insert("parentId".to_owned(), parent);
    Value::Object(fields)
}

/// The source id capture gives the `n`th line like `line` (with no id of its own) of the session
/// [`header`] names.
fn content_keyed(line: &Value, n: u32) -> String {
    use crate::harnesstools::session::Message as _;
    use crate::harnesstools::session::synthetic::{content_hash, synthetic_id};

    let session = SessionId::from(header(3)["id"].as_str().unwrap().to_owned());
    let message: PiMessage = crate::json::js::from_slice(line.to_string().as_bytes()).unwrap();
    assert_eq!(message.id(), None, "a line capture keys on its content");
    synthetic_id(content_hash(&session, &message), n)
}

/// A file pi converted from version 1 holds lines whose ids pi made up then, which capture never
/// keys them on: the copy reports each under the id capture gives it, keyed on its content and
/// counting identical lines as capture counts them, so a line of it sync hasn't got is never taken
/// for one it has. pi continues from such a line, which no row names: no tip.
#[rstest]
fn lines_pi_gave_ids_converting_a_file_are_keyed_as_capture_keys_them(dir: TempDir) {
    let prompt = migrated("0000aaaa", None, "user");
    let mut again = migrated("0000bbbb", Some("0000aaaa"), "user");
    again["message"] = prompt["message"].clone();
    let after = entry("aaaa0003", Some("0000bbbb"), "user");
    let path = write(&dir, &jsonl(&[header(3), prompt.clone(), again.clone(), after]));
    let tip = File::read(&path).unwrap().tip(&path);

    let mut want: HashSet<String> = [
        header(3)["id"].as_str().unwrap().to_owned(),
        content_keyed(&prompt, 0),
        content_keyed(&again, 1),
        "aaaa0003".to_owned(),
    ]
    .into();
    assert_eq!(content_keyed(&prompt, 0), content_keyed(&again, 0), "identical lines");
    assert_eq!(tip.known_source_ids, want);
    assert_eq!(tip.tip_source_id.as_deref(), Some("aaaa0003"));
    assert_eq!(tip.unswitchable, None);

    let path = write(&dir, &jsonl(&[header(3), prompt, again]));
    let tip = File::read(&path).unwrap().tip(&path);
    want.remove("aaaa0003");
    assert_eq!(tip.known_source_ids, want);
    assert_eq!(tip.tip_source_id, None, "pi continues from a line no row names");
}

/// A version 1 file (pi gives its entries new ids when it opens it) is never switched, and says
/// so up front.
#[rstest]
#[case::version_1(1, Some(OLD_FILE_SWITCH))]
#[case::version_3(3, None)]
fn an_old_file_is_unswitchable(dir: TempDir, #[case] version: u64, #[case] why: Option<&str>) {
    let path = write(&dir, &local(version));
    assert_eq!(File::read(&path).unwrap().tip(&path).unswitchable, why);
}

/// A row with no id (`syn-`, keyed on its content) of a pi session.
fn content_row(id: &str, role: Role, content: Vec<Content>) -> RehydrateMessage {
    RehydrateMessage {
        parent_source_id: None,
        role,
        content,
        ..row(id, "", Role::User)
    }
}

/// A branch holds the rows beside the tree that go with it (pi's prompts from before ids,
/// extensions' messages, titles: keyed on their content, with no parent): the switch passes over
/// those before where the branch leaves the copy, writes those after it it hasn't got (each
/// hanging from the entry before it, as a restore writes them; metadata merged into it), and pi
/// resumes from the last of them.
#[rstest]
fn a_switch_writes_the_rows_beside_the_tree_that_go_with_the_branch(dir: TempDir) {
    let backups = tempfile::tempdir().unwrap();
    let path = write(&dir, &jsonl(&went_its_own_way()));
    let base = File::read(&path).unwrap().tip(&path);
    let mut branch = their_branch();
    let said = |text: &str| vec![Content::Text(text.to_owned())];
    branch.insert(0, content_row("syn-0000000000000001", Role::User, said("before ids")));
    let custom = Role::Other("custom".into());
    branch.insert(5, content_row("syn-0000000000000002", custom, said("an extension's")));
    let title = Role::Other("session_info".into());
    branch.push(content_row("syn-0000000000000003", title, Vec::new()));

    let outcome = replace_to(&base, &branch, None, backups.path()).unwrap();

    assert_eq!(outcome.appended, ["bbbb0001", "syn-0000000000000002", "bbbb0002"]);
    assert_eq!(outcome.tip_source_id.as_deref(), Some("bbbb0002"));
    let after = File::read(&path).unwrap();
    let parents: HashMap<&str, Option<&str>> =
        after.entries.iter().map(|(id, parent, _)| (id.as_str(), parent.as_deref())).collect();
    assert_eq!(parents["syn-0000000000000002"], Some("bbbb0001"));
    let tip = after.tip(&path);
    assert!(tip.is_tip("syn-0000000000000003"), "the title merged into the last entry");
    assert!(!tip.known_source_ids.contains("syn-0000000000000001"));
}
