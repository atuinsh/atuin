#![allow(clippy::eq_op)]
mod common;

use self::common::*;
use eyre::{ensure, Result};

#[test]
fn test_messages() {
    assert_eq!("oh no!", bail_literal().unwrap_err().to_string());
    assert_eq!("oh no!", bail_fmt().unwrap_err().to_string());
    assert_eq!("oh no!", bail_error().unwrap_err().to_string());
}

#[test]
fn test_ensure() {
    let f = || -> Result<()> {
        ensure!(1 + 1 == 2, "This is correct");
        Ok(())
    };
    assert!(f().is_ok());

    let v = 1;
    let f = || -> Result<()> {
        ensure!(v + v == 2, "This is correct, v: {}", v);
        Ok(())
    };
    assert!(f().is_ok());

    let f = || -> Result<()> {
        ensure!(v + v == 1, "This is not correct, v: {}", v);
        Ok(())
    };
    assert!(f().is_err());
}
