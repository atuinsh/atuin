// Copyright 2018 Guillaume Pinot (@TeXitoi) <texitoi@texitoi.eu>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

// This should be in tests but it will not work until
// https://github.com/rust-lang/rust/issues/24584 is fixed

//! A test to check that structopt compiles with deny(missing_docs)

#![deny(missing_docs)]

use structopt::StructOpt;

/// The options
#[derive(StructOpt, Debug, PartialEq)]
pub struct Opt {
    #[structopt(short)]
    verbose: bool,
    #[structopt(subcommand)]
    cmd: Option<Cmd>,
}

/// Some subcommands
#[derive(StructOpt, Debug, PartialEq)]
pub enum Cmd {
    /// command A
    A,
    /// command B
    B {
        /// Alice?
        #[structopt(short)]
        alice: bool,
    },
    /// command C
    C(COpt),
}

/// The options for C
#[derive(StructOpt, Debug, PartialEq)]
pub struct COpt {
    #[structopt(short)]
    bob: bool,
}

fn main() {
    println!("{:?}", Opt::from_args());
}
