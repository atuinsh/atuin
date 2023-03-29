use eyre::{Context, Result};
use fs_err::create_dir_all;
use std::fs::File;

use tracing_subscriber::FmtSubscriber;

use daemonize::Daemonize;

use atuin_client::settings::Settings;
use tracing::{error, info, Level};

pub fn start(settings: Settings) -> Result<()> {
    // TODO: move this to be for the entire client
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::TRACE)
        // completes the builder.
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    // this means we can call this all over and not care about checking
    // just yeet it into shell bindings
    if !settings.daemon.enabled {
        return Ok(());
    }

    // honestly whoever thought of calling these demons did the coolest thing ever
    // THIS DIRECTORY IS CURSED
    let daemon_dir = atuin_common::utils::data_dir().join("daemon");
    let pid_file = daemon_dir.join("pid");

    let log = daemon_dir.join("atuin.log");
    let errlog = daemon_dir.join("atuin.err.log");

    let log = File::create(log)?;
    let errlog = File::create(errlog)?;

    create_dir_all(&daemon_dir).wrap_err_with(|| format!("could not create dir {daemon_dir:?}"))?;

    let daemonize = Daemonize::new()
        .pid_file(pid_file)
        .stdout(log)
        .stderr(errlog)
        .working_directory(daemon_dir);

    match daemonize.start() {
        Ok(_) => info!("Daemon booted up 😈"),
        Err(e) => error!("Error, {}", e),
    }

    Ok(())
}
