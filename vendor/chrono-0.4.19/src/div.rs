// This is a part of Chrono.
// Portions Copyright 2013-2014 The Rust Project Developers.
// See README.md and LICENSE.txt for details.

//! Integer division utilities. (Shamelessly copied from [num](https://github.com/rust-lang/num/))

// Algorithm from [Daan Leijen. _Division and Modulus for Computer Scientists_,
// December 2001](http://research.microsoft.com/pubs/151917/divmodnote-letter.pdf)

pub use num_integer::{div_floor, div_mod_floor, div_rem, mod_floor};

#[cfg(test)]
mod tests {
    use super::{div_mod_floor, mod_floor};

    #[test]
    fn test_mod_floor() {
        assert_eq!(mod_floor(8, 3), 2);
        assert_eq!(mod_floor(8, -3), -1);
        assert_eq!(mod_floor(-8, 3), 1);
        assert_eq!(mod_floor(-8, -3), -2);

        assert_eq!(mod_floor(1, 2), 1);
        assert_eq!(mod_floor(1, -2), -1);
        assert_eq!(mod_floor(-1, 2), 1);
        assert_eq!(mod_floor(-1, -2), -1);
    }

    #[test]
    fn test_div_mod_floor() {
        assert_eq!(div_mod_floor(8, 3), (2, 2));
        assert_eq!(div_mod_floor(8, -3), (-3, -1));
        assert_eq!(div_mod_floor(-8, 3), (-3, 1));
        assert_eq!(div_mod_floor(-8, -3), (2, -2));

        assert_eq!(div_mod_floor(1, 2), (0, 1));
        assert_eq!(div_mod_floor(1, -2), (-1, -1));
        assert_eq!(div_mod_floor(-1, 2), (-1, 1));
        assert_eq!(div_mod_floor(-1, -2), (0, -1));
    }
}
