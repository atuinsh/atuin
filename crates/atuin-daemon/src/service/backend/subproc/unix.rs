use std::path::PathBuf;

use daemonix::Daemonize;

use super::Subproc;

/// Errors from [`Subproc::daemonize`].
#[derive(Debug, thiserror::Error)]
pub enum DaemonizeError {
    #[error(transparent)]
    AtuinHome(#[from] atuin_common::dirs::HomeError),
    #[error("could not create the daemon's working directory {}: {source}", .path.display())]
    WorkingDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to daemonize process: {0}")]
    Daemonize(daemonix::Error),
}

impl Subproc {
    /// Detach this process, as the service `launch` started.
    ///
    /// Call it before creating the async runtime or opening database connections, since `fork()`
    /// inside an async runtime corrupts its internal state.
    pub fn daemonize() -> Result<(), DaemonizeError> {
        let working_directory = PathBuf::from(atuin_common::dirs::try_atuin_home()?);

        // TODO(markovejnovic): We should have some sort of DirsProvider or something like that
        //                      which would guarantee that the directory would exist.
        std::fs::create_dir_all(&working_directory).map_err(|source| {
            DaemonizeError::WorkingDir {
                path: working_directory.clone(),
                source,
            }
        })?;

        Daemonize::new()
            .working_directory(working_directory)
            .umask(0o077)
            .start()
            .map_err(DaemonizeError::Daemonize)
    }
}
