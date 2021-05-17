mod common;

use std::{env, fs};
use dotenv::*;

use crate::common::*;

#[test]
fn test_child_dir() {
    let dir = make_test_dotenv().unwrap();

    fs::create_dir("child").unwrap();

    env::set_current_dir("child").unwrap();

    dotenv().ok();
    assert_eq!(env::var("TESTKEY").unwrap(), "test_val");

    env::set_current_dir(dir.path().parent().unwrap()).unwrap();
    dir.close().unwrap();
}
