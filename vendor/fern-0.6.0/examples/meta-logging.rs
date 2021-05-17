//! This is an example to test the "meta-logging-in-format" fern cargo features.
//!
//! The example will hang if the feature is disabled, and will produce cohesive
//! logs if it's enabled.
use std::fmt;

use log::{debug, info};

fn main() {
    fern::Dispatch::new()
        .chain(std::io::stdout())
        .chain(std::io::stderr())
        .chain(fern::log_file("hello.txt").unwrap())
        .format(move |out, message, record| {
            out.finish(format_args!("[{}] {}", record.level(), message))
        })
        .apply()
        .unwrap();

    // in order to actually trigger the situation that deadlocks, we need a custom
    // Display implementation which performs logging:
    struct Thing<'a>(&'a str);

    impl<'a> fmt::Display for Thing<'a> {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            debug!("formatting Thing wrapping ({})", self.0);
            f.write_str(self.0)
        }
    }

    info!("I'm logging {}!", Thing("aha"));
}
