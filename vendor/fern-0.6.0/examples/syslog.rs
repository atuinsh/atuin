#[cfg(not(windows))]
// This is necessary because `fern` depends on both version 3 and 4.
use syslog4 as syslog;

use log::{debug, info, warn};

#[cfg(not(windows))]
fn setup_logging() -> Result<(), Box<dyn std::error::Error>> {
    let syslog_fmt = syslog::Formatter3164 {
        facility: syslog::Facility::LOG_USER,
        hostname: None,
        process: "fern-syslog-example".into(),
        pid: 0,
    };
    fern::Dispatch::new()
        // by default only accept warning messages so as not to spam
        .level(log::LevelFilter::Warn)
        // but accept Info if we explicitly mention it
        .level_for("explicit-syslog", log::LevelFilter::Info)
        .chain(syslog::unix(syslog_fmt)?)
        .apply()?;

    Ok(())
}

#[cfg(not(windows))]
fn main() {
    setup_logging().expect("failed to initialize logging.");

    // None of this will be shown in the syslog:
    for i in 0..5 {
        info!("executing section: {}", i);

        debug!("section {} 1/4 complete.", i);

        debug!("section {} 1/2 complete.", i);

        debug!("section {} 3/4 complete.", i);

        info!("section {} completed!", i);
    }

    // these two *will* show.

    info!(target: "explicit-syslog", "hello to the syslog! this is rust.");

    warn!("AHHH something's on fire.");
}

#[cfg(windows)]
fn main() {
    panic!("this example does not work on Windows.");
}
