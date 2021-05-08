use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use atuin_common::utils::uuid_v4;
use chrono::{TimeZone, Utc};
use directories::UserDirs;
use eyre::{eyre, Result};
use serde::Deserialize;

use super::{count_lines, Importer};
use crate::history::History;

#[derive(Deserialize, Debug)]
pub struct ReshEntry {
    #[serde(rename = "cmdLine")]
    pub cmd_line: String,
    #[serde(rename = "exitCode")]
    pub exit_code: i64,
    pub shell: String,
    pub uname: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub home: String,
    pub lang: String,
    #[serde(rename = "lcAll")]
    pub lc_all: String,
    pub login: String,
    pub pwd: String,
    #[serde(rename = "pwdAfter")]
    pub pwd_after: String,
    #[serde(rename = "shellEnv")]
    pub shell_env: String,
    pub term: String,
    #[serde(rename = "realPwd")]
    pub real_pwd: String,
    #[serde(rename = "realPwdAfter")]
    pub real_pwd_after: String,
    pub pid: i64,
    #[serde(rename = "sessionPid")]
    pub session_pid: i64,
    pub host: String,
    pub hosttype: String,
    pub ostype: String,
    pub machtype: String,
    pub shlvl: i64,
    #[serde(rename = "timezoneBefore")]
    pub timezone_before: String,
    #[serde(rename = "timezoneAfter")]
    pub timezone_after: String,
    #[serde(rename = "realtimeBefore")]
    pub realtime_before: f64,
    #[serde(rename = "realtimeAfter")]
    pub realtime_after: f64,
    #[serde(rename = "realtimeBeforeLocal")]
    pub realtime_before_local: f64,
    #[serde(rename = "realtimeAfterLocal")]
    pub realtime_after_local: f64,
    #[serde(rename = "realtimeDuration")]
    pub realtime_duration: f64,
    #[serde(rename = "realtimeSinceSessionStart")]
    pub realtime_since_session_start: f64,
    #[serde(rename = "realtimeSinceBoot")]
    pub realtime_since_boot: f64,
    #[serde(rename = "gitDir")]
    pub git_dir: String,
    #[serde(rename = "gitRealDir")]
    pub git_real_dir: String,
    #[serde(rename = "gitOriginRemote")]
    pub git_origin_remote: String,
    #[serde(rename = "gitDirAfter")]
    pub git_dir_after: String,
    #[serde(rename = "gitRealDirAfter")]
    pub git_real_dir_after: String,
    #[serde(rename = "gitOriginRemoteAfter")]
    pub git_origin_remote_after: String,
    #[serde(rename = "machineId")]
    pub machine_id: String,
    #[serde(rename = "osReleaseId")]
    pub os_release_id: String,
    #[serde(rename = "osReleaseVersionId")]
    pub os_release_version_id: String,
    #[serde(rename = "osReleaseIdLike")]
    pub os_release_id_like: String,
    #[serde(rename = "osReleaseName")]
    pub os_release_name: String,
    #[serde(rename = "osReleasePrettyName")]
    pub os_release_pretty_name: String,
    #[serde(rename = "reshUuid")]
    pub resh_uuid: String,
    #[serde(rename = "reshVersion")]
    pub resh_version: String,
    #[serde(rename = "reshRevision")]
    pub resh_revision: String,
    #[serde(rename = "partsMerged")]
    pub parts_merged: bool,
    pub recalled: bool,
    #[serde(rename = "recallLastCmdLine")]
    pub recall_last_cmd_line: String,
    pub cols: String,
    pub lines: String,
}

#[derive(Debug)]
pub struct Resh {
    file: BufReader<File>,
    strbuf: String,
    loc: u64,
    counter: i64,
}

impl Importer for Resh {
    const NAME: &'static str = "resh";

    fn histpath() -> Result<PathBuf> {
        let user_dirs = UserDirs::new().unwrap();
        let home_dir = user_dirs.home_dir();

        Ok(home_dir.join(".resh_history.json"))
    }

    fn parse(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path)?;
        let mut buf = BufReader::new(file);
        let loc = count_lines(&mut buf)?;

        Ok(Self {
            file: buf,
            strbuf: String::new(),
            loc: loc as u64,
            counter: 0,
        })
    }

    fn count(&self) -> u64 {
        self.loc
    }
}

impl Iterator for Resh {
    type Item = Result<History>;

    fn next(&mut self) -> Option<Self::Item> {
        self.strbuf.clear();
        match self.file.read_line(&mut self.strbuf) {
            Ok(0) => return None,
            Ok(_) => (),
            Err(e) => return Some(Err(eyre!("failed to read line: {}", e))), // we can skip past things like invalid utf8
        }

        let entry = match serde_json::from_str::<ReshEntry>(&self.strbuf) {
            Ok(e) => e,
            Err(e) => return Some(Err(eyre!("Invalid entry found in resh_history file: {}", e))),
        };

        #[allow(clippy::cast_possible_truncation)]
        #[allow(clippy::cast_sign_loss)]
        let timestamp = {
            let secs = entry.realtime_before.floor() as i64;
            let nanosecs = (entry.realtime_before.fract() * 1_000_000_000_f64).round() as u32;
            Utc.timestamp(secs, nanosecs)
        };
        #[allow(clippy::cast_possible_truncation)]
        #[allow(clippy::cast_sign_loss)]
        let duration = {
            let secs = entry.realtime_after.floor() as i64;
            let nanosecs = (entry.realtime_after.fract() * 1_000_000_000_f64).round() as u32;
            let difference = Utc.timestamp(secs, nanosecs) - timestamp;
            difference.num_nanoseconds().unwrap_or(0)
        };

        Some(Ok(History {
            id: uuid_v4(),
            timestamp,
            duration,
            exit: entry.exit_code,
            command: entry.cmd_line,
            cwd: entry.pwd,
            session: uuid_v4(),
            hostname: entry.host,
        }))
    }
}
