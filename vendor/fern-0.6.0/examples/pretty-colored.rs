//! This example shows how to configure fern to output really nicely colored
//! logs
//! - when the log level is error, the whole line is red
//! - when the log level is warn, the whole line is yellow
//! - when the log level is info, the level name is green and the rest of the
//!   line is white
//! - when the log level is debug, the whole line is white
//! - when the log level is trace, the whole line is gray ("bright black")
use fern::colors::{Color, ColoredLevelConfig};
use log::{debug, error, info, trace, warn};

fn main() {
    set_up_logging();
    // let's simulate some logging
    info!("starting simulation!");
    for i in 0..26 {
        trace!("loading: {}%, very verbose debbuging information", 4 * i);
        if 5 == i {
            debug!("this is taking so long... boooring!");
        } else if 10 == i {
            debug!("still alive! yay!");
        } else if 13 == i {
            info!("halfway there!");
        } else if 16 == i {
            debug!("*scratches nose*");
            warn!("nose is itching, continuing anyways");
        } else if 20 == i {
            debug!("uh oh");
            warn!(">nose itching intensifies");
            error!("HATCHOOO!");
            debug!("encountered minor problem, trying to recover");
            info!("gesundheit");
            debug!("recovered from minor problem, continuing");
        } else if 25 == i {
            info!("successfully loaded nothing");
            info!("have a good time!");
        }
    }
}

// ===================== Logging Set Up =====================
fn set_up_logging() {
    // configure colors for the whole line
    let colors_line = ColoredLevelConfig::new()
        .error(Color::Red)
        .warn(Color::Yellow)
        // we actually don't need to specify the color for debug and info, they are white by default
        .info(Color::White)
        .debug(Color::White)
        // depending on the terminals color scheme, this is the same as the background color
        .trace(Color::BrightBlack);

    // configure colors for the name of the level.
    // since almost all of them are the some as the color for the whole line, we
    // just clone `colors_line` and overwrite our changes
    let colors_level = colors_line.clone().info(Color::Green);
    // here we set up our fern Dispatch
    fern::Dispatch::new()
        .format(move |out, message, record| {
            out.finish(format_args!(
                "{color_line}[{date}][{target}][{level}{color_line}] {message}\x1B[0m",
                color_line = format_args!(
                    "\x1B[{}m",
                    colors_line.get_color(&record.level()).to_fg_str()
                ),
                date = chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                target = record.target(),
                level = colors_level.color(record.level()),
                message = message,
            ));
        })
        // set the default log level. to filter out verbose log messages from dependencies, set
        // this to Warn and overwrite the log level for your crate.
        .level(log::LevelFilter::Warn)
        // change log levels for individual modules. Note: This looks for the record's target
        // field which defaults to the module path but can be overwritten with the `target`
        // parameter:
        // `info!(target="special_target", "This log message is about special_target");`
        .level_for("pretty_colored", log::LevelFilter::Trace)
        // output to stdout
        .chain(std::io::stdout())
        .apply()
        .unwrap();

    debug!("finished setting up logging! yay!");
}
