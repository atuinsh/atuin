// Copyright 2018 Austin Bonander <austin.bonander@gmail.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
#![allow(missing_docs)]

mod std_buf;

#[cfg(feature = "slice-deque")]
mod slice_deque_buf;

use self::std_buf::StdBuf;

#[cfg(feature = "slice-deque")]
use self::slice_deque_buf::SliceDequeBuf;

pub enum BufImpl {
    Std(StdBuf),
    #[cfg(feature = "slice-deque")]
    Ringbuf(SliceDequeBuf),
}

macro_rules! forward_method {
    (pub fn $fnname:ident(&self $($args:tt)*) [$($passargs:tt)*] $(-> $ret:ty)*) => {
        pub fn $fnname(&self $($args)*) $(-> $ret)* {
            match *self {
                BufImpl::Std(ref buf) => buf.$fnname($($passargs)*),
                #[cfg(feature = "slice-deque")]
                BufImpl::Ringbuf(ref buf) => buf.$fnname($($passargs)*),
            }
        }
    };

    (pub fn $fnname:ident(&mut self $($args:tt)*) [$($passargs:tt)*] $(-> $ret:ty)*) => {
        pub fn $fnname(&mut self $($args)*) $(-> $ret)* {
            match *self {
                BufImpl::Std(ref mut buf) => buf.$fnname($($passargs)*),
                #[cfg(feature = "slice-deque")]
                BufImpl::Ringbuf(ref mut buf) => buf.$fnname($($passargs)*),
            }
        }
    };

    (pub unsafe fn $fnname:ident(&self $($args:tt)*) [$($passargs:tt)*] $(-> $ret:ty)*) => {
        pub unsafe fn $fnname(&self $($args)*) $(-> $ret)* {
            match *self {
                BufImpl::Std(ref buf) => buf.$fnname($($passargs)*),
                #[cfg(feature = "slice-deque")]
                BufImpl::Ringbuf(ref buf) => buf.$fnname($($passargs)*),
            }
        }
    };

    (pub unsafe fn $fnname:ident(&mut self $($args:tt)*) [$($passargs:tt)*] $(-> $ret:ty)*) => {
        pub unsafe fn $fnname(&mut self $($args)*) $(-> $ret)* {
            match *self {
                BufImpl::Std(ref mut buf) => buf.$fnname($($passargs)*),
                #[cfg(feature = "slice-deque")]
                BufImpl::Ringbuf(ref mut buf) => buf.$fnname($($passargs)*),
            }
        }
    };
}

macro_rules! forward_methods {
    ($($($qualifiers:ident)+ ($($args:tt)*) [$($passargs:tt)*] $(-> $ret:ty)*);+;) => (
        $(forward_method! {
            $($qualifiers)+ ($($args)*) [$($passargs)*] $(-> $ret)*
        })*
    )
}

impl BufImpl {
    pub fn with_capacity(cap: usize) -> Self {
        BufImpl::Std(StdBuf::with_capacity(cap))
    }

    #[cfg(feature = "slice-deque")]
    pub fn with_capacity_ringbuf(cap: usize) -> Self {
        BufImpl::Ringbuf(SliceDequeBuf::with_capacity(cap))
    }

    pub fn is_ringbuf(&self) -> bool {
        match *self {
            #[cfg(feature = "slice-deque")]
            BufImpl::Ringbuf(_) => true,
            _ => false,
        }
    }

    forward_methods! {
        pub fn capacity(&self)[] -> usize;

        pub fn len(&self)[] -> usize;

        pub fn usable_space(&self)[] -> usize;

        pub fn reserve(&mut self, additional: usize)[additional] -> bool;

        pub fn make_room(&mut self)[];

        pub fn buf(&self)[] -> &[u8];

        pub fn buf_mut(&mut self)[] -> &mut [u8];

        pub unsafe fn write_buf(&mut self)[] -> &mut [u8];

        pub unsafe fn bytes_written(&mut self, add: usize)[add];

        pub fn consume(&mut self, amt: usize)[amt];
    }
}
