// Copyright 2016-2018 Austin Bonander <austin.bonander@gmail.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Anything requiring unstable features (specialization, `Read::initializer()`, etc)

use std::fmt;
use std::io::{Read, Write};

use super::{BufReader, BufWriter, LineWriter};

use policy::{WriterPolicy, MoveStrategy, ReaderPolicy};

impl<R, Rs: ReaderPolicy> fmt::Debug for BufReader<R, Rs> {
    default fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("buf_redux::BufReader")
            .field("reader", &"(no Debug impl)")
            .field("available", &self.buf_len())
            .field("capacity", &self.capacity())
            .field("read_strategy", &self.read_strat)
            .field("move_strategy", &self.move_strat)
            .finish()
    }
}

impl<W: Write, Fs: WriterPolicy> fmt::Debug for BufWriter<W, Fs> {
    default fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("buf_redux::BufWriter")
            .field("writer", &"(no Debug impl)")
            .field("capacity", &self.capacity())
            .field("flush_strategy", &self.policy)
            .finish()
    }
}

impl<W: Write> fmt::Debug for LineWriter<W> {
    default fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("buf_redux::LineWriter")
            .field("writer", &"(no Debug impl)")
            .field("capacity", &self.capacity())
            .finish()
    }
}

pub fn init_buffer<R: Read + ?Sized>(rdr: &R, buf: &mut [u8]) {
    // no invariants for consumers to uphold:
    // https://doc.rust-lang.org/nightly/std/io/trait.Read.html#method.initializer
    unsafe { rdr.initializer().initialize(buf) }
}
