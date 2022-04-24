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
#[serde(rename_all = "camelCase")]
pub struct Entry {
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
    file: BufReader<File>,
    strbuf: String,
    loc: usize,
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
            loc,
        })
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

        // .resh_history.json lies about being a json. It is actually a file containing valid json
        // on every line. This means that the last line will throw an error, as it is just an EOF.
        // Without the special case here, that will crash the importer.
        let entry = match serde_json::from_str::<Entry>(&self.strbuf) {
            Ok(e) => e,
            Err(e) if e.is_eof() => return None,
            Err(e) => {
                return Some(Err(eyre!(
                    "Invalid entry found in resh_history file: {}",
                    e
                )))
            }
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

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.loc, Some(self.loc))
    }
}
