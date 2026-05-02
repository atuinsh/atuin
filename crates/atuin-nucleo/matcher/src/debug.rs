use crate::matrix::{MatrixCell, ScoreCell};
use std::fmt::{Debug, Formatter, Result};

impl Debug for ScoreCell {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        write!(f, "({}, {})", self.score, self.matched)
    }
}

impl Debug for MatrixCell {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        write!(f, "({}, {})", (self.0 & 1) != 0, (self.0 & 2) != 0)
    }
}
