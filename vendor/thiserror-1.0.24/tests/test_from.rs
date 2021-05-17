#![deny(clippy::all, clippy::pedantic)]

use std::io;
use thiserror::Error;

#[derive(Error, Debug)]
#[error("...")]
pub struct ErrorStruct {
    #[from]
    source: io::Error,
}

#[derive(Error, Debug)]
#[error("...")]
pub struct ErrorTuple(#[from] io::Error);

#[derive(Error, Debug)]
#[error("...")]
pub enum ErrorEnum {
    Test {
        #[from]
        source: io::Error,
    },
}

#[derive(Error, Debug)]
#[error("...")]
pub enum Many {
    Any(#[from] anyhow::Error),
    Io(#[from] io::Error),
}

fn assert_impl<T: From<io::Error>>() {}

#[test]
fn test_from() {
    assert_impl::<ErrorStruct>();
    assert_impl::<ErrorTuple>();
    assert_impl::<ErrorEnum>();
    assert_impl::<Many>();
}
