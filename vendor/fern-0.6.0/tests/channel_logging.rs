//! Tests!
use log::Level::*;

mod support;

use support::manual_log;

#[test]
fn test_channel_logging() {
    use std::sync::mpsc;
    // Create the channel
    let (send, recv) = mpsc::channel();

    let (_max_level, logger) = fern::Dispatch::new().chain(send).into_log();

    let l = &*logger;
    manual_log(l, Info, "message1");
    manual_log(l, Info, "message2");

    logger.flush();

    assert_eq!(recv.recv().unwrap(), "message1\n");
    assert_eq!(recv.recv().unwrap(), "message2\n");
}
