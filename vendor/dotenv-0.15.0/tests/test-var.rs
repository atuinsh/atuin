mod common;

use std::env;

use dotenv::*;

use crate::common::*;

#[test]
fn test_var() {
    let dir = make_test_dotenv().unwrap();

    assert_eq!(var("TESTKEY").unwrap(), "test_val");

    env::set_current_dir(dir.path().parent().unwrap()).unwrap();
    dir.close().unwrap();
}
