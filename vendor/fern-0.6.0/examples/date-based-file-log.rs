use log::{debug, info, warn};

fn setup_logging() -> Result<(), Box<dyn std::error::Error>> {
    fern::Dispatch::new()
        .level(log::LevelFilter::Debug)
        .chain(fern::DateBased::new("program.log.", "%Y-%m-%d"))
        .apply()?;

    Ok(())
}

fn main() {
    setup_logging().expect("failed to initialize logging.");

    for i in 0..5 {
        info!("executing section: {}", i);

        debug!("section {} 1/4 complete.", i);

        debug!("section {} 1/2 complete.", i);

        debug!("section {} 3/4 complete.", i);

        info!("section {} completed!", i);
    }

    warn!("AHHH something's on fire.");
}
