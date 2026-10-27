//! The daily log file every process appends to.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use tracing_appender::rolling::{InitError, RollingFileAppender, Rotation};

/// The log directory's daily files, like `atuin.2026-10-20.log`.
///
/// Every process appends to the same file: it is opened in append mode and the formatter writes
/// each event with one `write`, so lines from concurrent processes interleave whole. Only the date
/// in the name rotates it, so no process renames a file another one still has open.
pub(super) struct LogFile {
    dir: PathBuf,
}

impl LogFile {
    /// How long a log file is kept after its last write.
    const RETENTION: Duration = Duration::from_hours(4 * 24);

    /// File names older atuin versions wrote, cleaned up like today's once they go stale.
    const LEGACY_PREFIXES: [&str; 3] = ["search.log.", "daemon.log.", "ai.log."];

    pub(super) const fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Open today's file for appending, creating the directory if needed.
    pub(super) fn appender(&self) -> Result<RollingFileAppender, InitError> {
        RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .filename_prefix("atuin")
            .filename_suffix("log")
            .build(&self.dir)
    }

    /// Remove the log files last written more than [`Self::RETENTION`] before `now`.
    ///
    /// Silent on every error: concurrent processes prune the same directory, and a file another one
    /// already removed is no problem.
    pub(super) fn prune(&self, now: SystemTime) {
        let Some(cutoff) = now.checked_sub(Self::RETENTION) else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };

        entries
            .flatten()
            .filter(|entry| entry.file_name().to_str().is_some_and(Self::is_log))
            .filter(|entry| {
                entry.metadata().is_ok_and(|meta| {
                    meta.is_file() && meta.modified().is_ok_and(|modified| modified < cutoff)
                })
            })
            .for_each(|entry| {
                let _ = std::fs::remove_file(entry.path());
            });
    }

    fn is_log(name: &str) -> bool {
        name.strip_prefix("atuin.").is_some_and(|rest| rest.strip_suffix(".log").is_some())
            || Self::LEGACY_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::time::{Duration, SystemTime};

    use rstest::rstest;

    use super::LogFile;

    const DAY: Duration = Duration::from_hours(24);

    #[rstest]
    #[case::stale_atuin("atuin.2026-10-10.log", 5, false)]
    #[case::fresh_atuin("atuin.2026-10-19.log", 1, true)]
    #[case::stale_daemon("daemon.log.2026-10-10", 5, false)]
    #[case::stale_search("search.log.2026-10-10", 5, false)]
    #[case::stale_ai("ai.log.2026-10-10", 5, false)]
    #[case::foreign("notes.txt", 5, true)]
    fn prune_removes_only_stale_logs(
        #[case] name: &str,
        #[case] age_days: u32,
        #[case] kept: bool,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        let now = SystemTime::now();
        File::create(&path).unwrap().set_modified(now - DAY * age_days).unwrap();

        LogFile::new(dir.path().to_path_buf()).prune(now);

        assert_eq!(path.exists(), kept);
    }

    #[rstest]
    fn prune_without_directory_is_silent() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");

        LogFile::new(missing.clone()).prune(SystemTime::now());

        assert!(!missing.exists());
    }

    #[rstest]
    fn appender_fails_when_directory_is_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let not_a_dir = dir.path().join("logs");
        File::create(&not_a_dir).unwrap();

        assert!(LogFile::new(not_a_dir).appender().is_err());
    }
}
