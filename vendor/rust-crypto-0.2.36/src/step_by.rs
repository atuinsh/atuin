// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

/// This module just implements a simple verison of step_by() since
/// the function from the standard library is currently unstable.
/// This should be removed once that function becomes stable.

use std::ops::{Add, Range};

#[derive(Clone)]
pub struct StepUp<T> {
    next: T,
    end: T,
    ammount: T
}

impl <T> Iterator for StepUp<T> where
        T: Add<T, Output = T> + PartialOrd + Copy {
    type Item = T;

    #[inline]
    fn next(&mut self) -> Option<T> {
        if self.next < self.end {
            let n = self.next;
            self.next = self.next + self.ammount;
            Some(n)
        } else {
            None
        }
    }
}

pub trait RangeExt<T> {
    fn step_up(self, ammount: T) -> StepUp<T>;
}

impl <T> RangeExt<T> for Range<T> where
        T: Add<T, Output = T> + PartialOrd + Copy {
    fn step_up(self, ammount: T) -> StepUp<T> {
        StepUp {
            next: self.start,
            end: self.end,
            ammount: ammount
        }
    }
}

