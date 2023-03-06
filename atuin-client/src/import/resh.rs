use std::{
    fs,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use directories::UserDirs;
use eyre::{bail, Result};
use serde::Deserialize;

use atuin_common::utils::uuid_v4;

use super::{unix_byte_lines, Importer, Loader};
use crate::history::History;

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

#[async_trait]
impl Importer for Resh {
    const NAME: &'static str = "resh";

    fn default_source_path() -> Result<PathBuf> {
        let Some(user_dirs) = UserDirs::new() else {
            bail!("could not find user directories");
        };
        let path = user_dirs.home_dir().join(".resh_history.json");

        Ok(path)
    }

    async fn new(source: &Path) -> Result<Self> {
        let bytes = fs::read(source)?;
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

            h.push(History {
                id: uuid_v4(),
                timestamp,
                duration,
                exit: entry.exit_code,
                command: entry.cmd_line,
                cwd: entry.pwd,
                session: uuid_v4(),
                hostname: entry.host,
            })
            .await?;
        }

        Ok(())
    }
}
