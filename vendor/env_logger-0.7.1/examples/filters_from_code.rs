/*!
Specify logging filters in code instead of using an environment variable.
*/

#[macro_use]
extern crate log;

fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Trace)
        .init();

    trace!("some trace log");
    debug!("some debug log");
    info!("some information log");
    warn!("some warning log");
    error!("some error log");
}
