use std::path::PathBuf;

use async_trait::async_trait;
use directories::UserDirs;
use eyre::{Result, eyre};
use serde::Deserialize;

use atuin_common::utils::uuid_v7;
use time::OffsetDateTime;

use super::{Importer, Loader, get_histfile_path, timestamp_from_parts, unix_byte_lines};
use crate::history::History;
use crate::history::builder::HistoryImported;
use crate::import::read_to_end;

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ReshEntry {
    pub cmd_line: String,
    pub exit_code: i64,
    pub shell: String,
    pub uname: String,
    pub session_id: String,
    pub home: String,
    pub lang: String,
    pub lc_all: String,
    pub login: String,
    pub pwd: String,
    pub pwd_after: String,
    pub shell_env: String,
    pub term: String,
    pub real_pwd: String,
    pub real_pwd_after: String,
    pub pid: i64,
    pub session_pid: i64,
    pub host: String,
    pub hosttype: String,
    pub ostype: String,
    pub machtype: String,
    pub shlvl: i64,
    pub timezone_before: String,
    pub timezone_after: String,
    pub realtime_before: f64,
    pub realtime_after: f64,
    pub realtime_before_local: f64,
    pub realtime_after_local: f64,
    pub realtime_duration: f64,
    pub realtime_since_session_start: f64,
    pub realtime_since_boot: f64,
    pub git_dir: String,
    pub git_real_dir: String,
    pub git_origin_remote: String,
    pub git_dir_after: String,
    pub git_real_dir_after: String,
    pub git_origin_remote_after: String,
    pub machine_id: String,
    pub os_release_id: String,
    pub os_release_version_id: String,
    pub os_release_id_like: String,
    pub os_release_name: String,
    pub os_release_pretty_name: String,
    pub resh_uuid: String,
    pub resh_version: String,
    pub resh_revision: String,
    pub parts_merged: bool,
    pub recalled: bool,
    pub recall_last_cmd_line: String,
    pub cols: String,
    pub lines: String,
}

#[derive(Debug)]
pub struct Resh {
    bytes: Vec<u8>,
}

fn default_histpath() -> Result<PathBuf> {
    let user_dirs = UserDirs::new().ok_or_else(|| eyre!("could not find user directories"))?;
    let home_dir = user_dirs.home_dir();

    Ok(home_dir.join(".resh_history.json"))
}

#[async_trait]
impl Importer for Resh {
    const NAME: &'static str = "resh";

    async fn new() -> Result<Self> {
        let bytes = read_to_end(get_histfile_path(default_histpath)?)?;
        Ok(Self { bytes })
    }

    async fn entries(&mut self) -> Result<usize> {
        Ok(super::count_lines(&self.bytes))
    }

    async fn load(self, h: &mut impl Loader) -> Result<()> {
        for b in unix_byte_lines(&self.bytes) {
            let s = match std::str::from_utf8(b) {
                Ok(s) => s,
                Err(_) => continue, // we can skip past things like invalid utf8
            };
            let entry = match serde_json::from_str::<ReshEntry>(s) {
                Ok(e) => e,
                Err(_) => continue, // skip invalid json :shrug:
            };

            #[allow(clippy::cast_possible_truncation)]
            #[allow(clippy::cast_sign_loss)]
            let start = {
                let secs = entry.realtime_before.floor() as i64;
                let nanosecs = (entry.realtime_before.fract() * 1_000_000_000_f64).round() as i64;
                timestamp_from_parts(secs, nanosecs)
            };
            #[allow(clippy::cast_possible_truncation)]
            #[allow(clippy::cast_sign_loss)]
            let end = {
                let secs = entry.realtime_after.floor() as i64;
                let nanosecs = (entry.realtime_after.fract() * 1_000_000_000_f64).round() as i64;
                timestamp_from_parts(secs, nanosecs)
            };

            // a corrupt entry must not abort the whole import. only report a duration when
            // both ends are representable - measuring against the epoch sentinel would
            // invent a decades-long duration. clock skew, an NTP step, or suspend/resume
            // can also make realtime_after precede realtime_before; a negative duration is
            // just as meaningless as an unrepresentable one, so it falls back the same way
            let duration = match (start, end) {
                (Some(start), Some(end)) => {
                    match i64::try_from((end - start).whole_nanoseconds()) {
                        Ok(nanos) if nanos >= 0 => nanos,
                        _ => HistoryImported::DEFAULT_DURATION,
                    }
                }
                _ => HistoryImported::DEFAULT_DURATION,
            };
            let timestamp = start.unwrap_or(OffsetDateTime::UNIX_EPOCH);

            let imported = History::import()
                // resh shell string matches what we use; see
                // https://github.com/curusarn/resh/blob/master/scripts/shellrc.sh
                .shell(entry.shell)
                .command(entry.cmd_line)
                .timestamp(timestamp)
                .duration(duration)
                .exit(entry.exit_code)
                .cwd(entry.pwd)
                .hostname(entry.host)
                // CHECK: should we add uuid here? It's not set in the other importers
                .session(uuid_v7().as_simple().to_string());

            h.push(imported.build().into()).await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::import::tests::TestLoader;

    /// resh writes one JSON object per line. Every field on `ReshEntry` is
    /// required, so spell them all out once here.
    ///
    /// Built field-by-field (rather than one big `serde_json::json!{...}`
    /// call) because a single macro invocation with all 51 fields blows the
    /// default macro recursion limit.
    fn resh_line(cmd: &str, realtime_before: f64, realtime_after: f64) -> String {
        let mut m = serde_json::Map::new();
        m.insert("cmdLine".into(), serde_json::json!(cmd));
        m.insert("exitCode".into(), serde_json::json!(0));
        m.insert("shell".into(), serde_json::json!("bash"));
        m.insert("uname".into(), serde_json::json!("Linux"));
        m.insert("sessionId".into(), serde_json::json!("s"));
        m.insert("home".into(), serde_json::json!("/root"));
        m.insert("lang".into(), serde_json::json!("C"));
        m.insert("lcAll".into(), serde_json::json!("C"));
        m.insert("login".into(), serde_json::json!("root"));
        m.insert("pwd".into(), serde_json::json!("/tmp"));
        m.insert("pwdAfter".into(), serde_json::json!("/tmp"));
        m.insert("shellEnv".into(), serde_json::json!(""));
        m.insert("term".into(), serde_json::json!("xterm"));
        m.insert("realPwd".into(), serde_json::json!("/tmp"));
        m.insert("realPwdAfter".into(), serde_json::json!("/tmp"));
        m.insert("pid".into(), serde_json::json!(1));
        m.insert("sessionPid".into(), serde_json::json!(1));
        m.insert("host".into(), serde_json::json!("box"));
        m.insert("hosttype".into(), serde_json::json!("x86_64"));
        m.insert("ostype".into(), serde_json::json!("linux"));
        m.insert("machtype".into(), serde_json::json!("x86_64"));
        m.insert("shlvl".into(), serde_json::json!(1));
        m.insert("timezoneBefore".into(), serde_json::json!("+0000"));
        m.insert("timezoneAfter".into(), serde_json::json!("+0000"));
        m.insert("realtimeBefore".into(), serde_json::json!(realtime_before));
        m.insert("realtimeAfter".into(), serde_json::json!(realtime_after));
        m.insert(
            "realtimeBeforeLocal".into(),
            serde_json::json!(realtime_before),
        );
        m.insert(
            "realtimeAfterLocal".into(),
            serde_json::json!(realtime_after),
        );
        m.insert("realtimeDuration".into(), serde_json::json!(0.0));
        m.insert("realtimeSinceSessionStart".into(), serde_json::json!(0.0));
        m.insert("realtimeSinceBoot".into(), serde_json::json!(0.0));
        m.insert("gitDir".into(), serde_json::json!(""));
        m.insert("gitRealDir".into(), serde_json::json!(""));
        m.insert("gitOriginRemote".into(), serde_json::json!(""));
        m.insert("gitDirAfter".into(), serde_json::json!(""));
        m.insert("gitRealDirAfter".into(), serde_json::json!(""));
        m.insert("gitOriginRemoteAfter".into(), serde_json::json!(""));
        m.insert("machineId".into(), serde_json::json!(""));
        m.insert("osReleaseId".into(), serde_json::json!(""));
        m.insert("osReleaseVersionId".into(), serde_json::json!(""));
        m.insert("osReleaseIdLike".into(), serde_json::json!(""));
        m.insert("osReleaseName".into(), serde_json::json!(""));
        m.insert("osReleasePrettyName".into(), serde_json::json!(""));
        m.insert("reshUuid".into(), serde_json::json!(""));
        m.insert("reshVersion".into(), serde_json::json!(""));
        m.insert("reshRevision".into(), serde_json::json!(""));
        m.insert("partsMerged".into(), serde_json::json!(false));
        m.insert("recalled".into(), serde_json::json!(false));
        m.insert("recallLastCmdLine".into(), serde_json::json!(""));
        m.insert("cols".into(), serde_json::json!("80"));
        m.insert("lines".into(), serde_json::json!("24"));

        serde_json::Value::Object(m).to_string()
    }

    #[tokio::test]
    async fn out_of_range_timestamp_falls_back_to_epoch() {
        // one good entry, one whose realtime is far outside the representable range
        let bytes = format!(
            "{}\n{}\n",
            resh_line("echo good", 1_639_162_832.5, 1_639_162_833.5),
            resh_line("echo corrupt", 1e30, 1e30),
        )
        .into_bytes();

        let resh = Resh { bytes };
        let mut loader = TestLoader::default();
        resh.load(&mut loader).await.expect("import must not fail");

        let commands: Vec<&str> = loader.buf.iter().map(|h| h.command.as_str()).collect();
        assert_eq!(commands, ["echo good", "echo corrupt"]);

        assert_eq!(loader.buf[0].timestamp.unix_timestamp(), 1_639_162_832);
        assert_eq!(loader.buf[1].timestamp, OffsetDateTime::UNIX_EPOCH);
        assert_eq!(loader.buf[1].duration, HistoryImported::DEFAULT_DURATION);
    }

    #[tokio::test]
    async fn corrupt_realtime_before_falls_back_to_epoch_and_default_duration() {
        let bytes = format!("{}\n", resh_line("echo corrupt", 1e30, 1_639_162_833.5)).into_bytes();

        let resh = Resh { bytes };
        let mut loader = TestLoader::default();
        resh.load(&mut loader).await.expect("import must not fail");

        let commands: Vec<&str> = loader.buf.iter().map(|h| h.command.as_str()).collect();
        assert_eq!(commands, ["echo corrupt"]);

        assert_eq!(loader.buf[0].timestamp, OffsetDateTime::UNIX_EPOCH);
        assert_eq!(loader.buf[0].duration, HistoryImported::DEFAULT_DURATION);
    }

    #[tokio::test]
    async fn corrupt_realtime_after_keeps_real_timestamp_but_falls_back_to_default_duration() {
        let bytes = format!("{}\n", resh_line("echo corrupt", 1_639_162_832.5, 1e30)).into_bytes();

        let resh = Resh { bytes };
        let mut loader = TestLoader::default();
        resh.load(&mut loader).await.expect("import must not fail");

        let commands: Vec<&str> = loader.buf.iter().map(|h| h.command.as_str()).collect();
        assert_eq!(commands, ["echo corrupt"]);

        assert_eq!(loader.buf[0].timestamp.unix_timestamp(), 1_639_162_832);
        assert_eq!(loader.buf[0].duration, HistoryImported::DEFAULT_DURATION);
    }

    #[tokio::test]
    async fn clock_skew_negative_duration_falls_back_to_default_duration() {
        // realtime_after earlier than realtime_before (clock skew, NTP step, or
        // suspend/resume) must not produce a negative duration
        let bytes = format!(
            "{}\n",
            resh_line("echo skewed", 1_639_162_833.5, 1_639_162_832.5)
        )
        .into_bytes();

        let resh = Resh { bytes };
        let mut loader = TestLoader::default();
        resh.load(&mut loader).await.expect("import must not fail");

        let commands: Vec<&str> = loader.buf.iter().map(|h| h.command.as_str()).collect();
        assert_eq!(commands, ["echo skewed"]);

        assert_eq!(loader.buf[0].timestamp.unix_timestamp(), 1_639_162_833);
        assert_eq!(loader.buf[0].duration, HistoryImported::DEFAULT_DURATION);
    }
}
