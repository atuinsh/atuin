use std::env;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use atuin_common::time::OffsetDateTimeExt;
use atuin_domain::record::CmdOrigin;
use directories::BaseDirs;
use easy_cast::{CastTo, Trunc};
use eyre::{Result, eyre};
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;
use uuid::timestamp::Timestamp;
use uuid::timestamp::context::NoContext;

use super::{Importer, Loader, get_histdir_path};
use crate::history::History;
use crate::history::builder::HistoryImported;

// Note: both HistoryFile and HistoryData have other keys present in the JSON, we don't
// care about them so we leave them unspecified so as to avoid deserializing unnecessarily.
#[derive(Debug, Deserialize)]
struct HistoryFile {
    data: HistoryData,
}

#[derive(Debug, Deserialize)]
struct HistoryData {
    sessionid: String,
    cmds: Vec<HistoryCmd>,
}

#[derive(Debug, Deserialize)]
struct HistoryCmd {
    cwd: String,
    inp: String,
    rtn: Option<i64>,
    ts: (f64, f64),
}

#[derive(Debug)]
pub struct Xonsh {
    // history is stored as a bunch of json files, one per session
    sessions: Vec<HistoryData>,
    cmd_origin: CmdOrigin,
}

fn xonsh_hist_dir(xonsh_data_dir: Option<String>) -> Result<PathBuf> {
    // if running within xonsh, this will be available
    if let Some(d) = xonsh_data_dir {
        let mut path = PathBuf::from(d);
        path.push("history_json");
        return Ok(path);
    }

    // otherwise, fall back to default
    let base = BaseDirs::new().ok_or_else(|| eyre!("Could not determine home directory"))?;

    let hist_dir = base.data_dir().join("xonsh/history_json");
    if hist_dir.exists() || cfg!(test) {
        Ok(hist_dir)
    } else {
        Err(eyre!("Could not find xonsh history files"))
    }
}

fn load_sessions(hist_dir: &Path) -> Result<Vec<HistoryData>> {
    let mut sessions = vec![];
    for entry in fs::read_dir(hist_dir)? {
        let p = entry?.path();
        let ext = p.extension().and_then(|e| e.to_str());
        if p.is_file()
            && ext == Some("json")
            && let Some(data) = load_session(&p)?
        {
            sessions.push(data);
        }
    }
    Ok(sessions)
}

fn load_session(path: &Path) -> Result<Option<HistoryData>> {
    let file = File::open(path)?;
    // empty files are not valid json, so we can't deserialize them
    if file.metadata()?.len() == 0 {
        return Ok(None);
    }

    let mut hist_file: HistoryFile = serde_json::from_reader(file)?;

    // if there are commands in this session, replace the existing UUIDv4
    // with a UUIDv7 generated from the timestamp of the first command
    if let Some(cmd) = hist_file.data.cmds.first() {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "only used for creating a UUID -- saturating is ok"
        )]
        let (seconds, nanos) =
            (cmd.ts.0.trunc() as u64, (cmd.ts.0.fract() * 1_000_000_000_f64) as u32);
        let ts = Timestamp::from_unix(NoContext, seconds, nanos);
        hist_file.data.sessionid = Uuid::new_v7(ts).to_string();
    }
    Ok(Some(hist_file.data))
}

#[async_trait]
impl Importer for Xonsh {
    const NAME: &'static str = "xonsh";

    async fn new() -> Result<Self> {
        // wrap xonsh-specific path resolver in general one so that it respects $HISTPATH
        let xonsh_data_dir = env::var("XONSH_DATA_DIR").ok();
        let hist_dir = get_histdir_path(|| xonsh_hist_dir(xonsh_data_dir))?;
        let sessions = load_sessions(&hist_dir)?;
        let cmd_origin = CmdOrigin::probe_current();
        Ok(Self {
            sessions,
            cmd_origin,
        })
    }

    async fn entries(&mut self) -> Result<usize> {
        let total = self.sessions.iter().map(|s| s.cmds.len()).sum();
        Ok(total)
    }

    async fn load(self, loader: &mut impl Loader) -> Result<()> {
        for session in self.sessions {
            for cmd in session.cmds {
                let (start, end) = cmd.ts;
                let timestamp = (start * 1_000_000_000_f64)
                    .try_cast_to(Trunc)
                    .ok()
                    .and_then(|nanos: i128| OffsetDateTime::from_unix_nanos(nanos).ok())
                    .unwrap_or(OffsetDateTime::UNIX_EPOCH);

                let duration = ((end - start) * 1_000_000_000_f64)
                    .try_cast_to(Trunc)
                    .unwrap_or(HistoryImported::DEFAULT_DURATION);

                let entry = History::import()
                    .shell("xonsh")
                    .timestamp(timestamp)
                    .duration(duration)
                    .exit(cmd.rtn.unwrap_or(HistoryImported::DEFAULT_EXIT))
                    .command(cmd.inp.trim())
                    .cwd(cmd.cwd)
                    .session(session.sessionid.clone())
                    .cmd_origin(self.cmd_origin.clone());
                loader.push(entry.build().into()).await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;
    use crate::history::History;
    use crate::import::tests::TestLoader;

    #[test]
    fn test_hist_dir_xonsh() {
        let hist_dir = xonsh_hist_dir(Some("/home/user/xonsh_data".to_string())).unwrap();
        assert_eq!(hist_dir, PathBuf::from("/home/user/xonsh_data/history_json"));
    }

    #[tokio::test]
    async fn out_of_range_timestamp_falls_back_to_epoch() {
        let xonsh = Xonsh {
            sessions: vec![HistoryData {
                sessionid: "8ea62c1c-cf1d-4d7f-8b03-f6d7d02b4f9b".to_string(),
                cmds: vec![HistoryCmd {
                    cwd: "/tmp".to_string(),
                    inp: "echo hello".to_string(),
                    rtn: Some(0),
                    ts: (1e30, 1e30),
                }],
            }],
            cmd_origin: CmdOrigin::try_from("box:user").unwrap(),
        };

        let mut loader = TestLoader::default();
        xonsh.load(&mut loader).await.expect("import must not fail");

        assert_eq!(loader.buf.len(), 1);
        assert_eq!(loader.buf[0].timestamp, OffsetDateTime::UNIX_EPOCH);
        assert_eq!(loader.buf[0].command, "echo hello");
    }

    #[tokio::test]
    async fn test_import() {
        let dir = PathBuf::from("tests/data/xonsh");
        let sessions = load_sessions(&dir).unwrap();
        let cmd_origin = CmdOrigin::try_from("box:user").unwrap();
        let xonsh = Xonsh {
            sessions,
            cmd_origin,
        };

        let mut loader = TestLoader::default();
        xonsh.load(&mut loader).await.unwrap();
        // order in buf will depend on filenames, so sort by timestamp for consistency
        loader.buf.sort_by_key(|h| h.timestamp);
        for (actual, expected) in loader.buf.iter().zip(expected_hist_entries().iter()) {
            assert_eq!(actual.timestamp, expected.timestamp);
            assert_eq!(actual.command, expected.command);
            assert_eq!(actual.cwd, expected.cwd);
            assert_eq!(actual.exit, expected.exit);
            assert_eq!(actual.duration, expected.duration);
            assert_eq!(actual.cmd_origin, expected.cmd_origin);
        }
    }

    fn expected_hist_entries() -> [History; 4] {
        [
            History::import()
                .timestamp(datetime!(2024-02-6 04:17:59.478272256 +00:00:00))
                .command("echo hello world!".to_string())
                .cwd("/home/user/Documents/code/atuin".to_string())
                .exit(0)
                .duration(4_651_069)
                .cmd_origin(CmdOrigin::try_from("box:user").unwrap())
                .build()
                .into(),
            History::import()
                .timestamp(datetime!(2024-02-06 04:18:01.70632832 +00:00:00))
                .command("ls -l".to_string())
                .cwd("/home/user/Documents/code/atuin".to_string())
                .exit(0)
                .duration(21_288_633)
                .cmd_origin(CmdOrigin::try_from("box:user").unwrap())
                .build()
                .into(),
            History::import()
                .timestamp(datetime!(2024-02-06 17:41:31.142515968 +00:00:00))
                .command("false".to_string())
                .cwd("/home/user/Documents/code/atuin/atuin-client".to_string())
                .exit(1)
                .duration(10_269_403)
                .cmd_origin(CmdOrigin::try_from("box:user").unwrap())
                .build()
                .into(),
            History::import()
                .timestamp(datetime!(2024-02-06 17:41:32.271584 +00:00:00))
                .command("exit".to_string())
                .cwd("/home/user/Documents/code/atuin/atuin-client".to_string())
                .exit(0)
                .duration(4_259_347)
                .cmd_origin(CmdOrigin::try_from("box:user").unwrap())
                .build()
                .into(),
        ]
    }
}
