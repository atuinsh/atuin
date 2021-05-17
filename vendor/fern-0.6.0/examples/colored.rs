use fern::colors::{Color, ColoredLevelConfig};
use log::{debug, error, warn};

fn main() {
    let colors = ColoredLevelConfig::new().debug(Color::Magenta);

    fern::Dispatch::new()
        .chain(std::io::stdout())
        .format(move |out, message, record| {
            out.finish(format_args!(
                "[{}]{} {}",
                // This will color the log level only, not the whole line. Just a touch.
                colors.color(record.level()),
                chrono::Utc::now().format("[%Y-%m-%d %H:%M:%S]"),
                message
            ))
        })
        .apply()
        .unwrap();

    error!("hi");
    debug!("sup");
    warn!("oh");
}
