// Copyright 2015-2016 Brian Smith.
//
// Permission to use, copy, modify, and/or distribute this software for any
// purpose with or without fee is hereby granted, provided that the above
// copyright notice and this permission notice appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHORS DISCLAIM ALL WARRANTIES
// WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
// MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR
// ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
// ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
// OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

//! Conversions into the library's time type.

#[cfg(feature = "std")]
use {ring, std};

/// The time type.
///
/// Internally this is merely a UNIX timestamp: a count of non-leap
/// seconds since the start of 1970.  This type exists to assist
/// unit-of-measure correctness.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Time(u64);

impl Time {
    /// Create a `webpki::Time` from a `std::time::SystemTime`.
    ///
    /// This will be replaced with a real `TryFrom<std::time::SystemTime>`
    /// implementation when `TryFrom` is added to Rust Stable.
    ///
    /// # Example:
    ///
    /// Construct a `webpki::Time` from the current system time:
    ///
    /// ```
    /// # extern crate ring;
    /// # extern crate webpki;
    /// #
    /// #[cfg(feature = "std")]
    /// # fn foo() -> Result<(), ring::error::Unspecified> {
    /// let time = webpki::Time::try_from(std::time::SystemTime::now())?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "std")]
    pub fn try_from(time: std::time::SystemTime) -> Result<Time, ring::error::Unspecified> {
        time.duration_since(std::time::UNIX_EPOCH)
            .map(|d| Time::from_seconds_since_unix_epoch(d.as_secs()))
            .map_err(|_| ring::error::Unspecified)
    }

    /// Create a `webpki::Time` from a unix timestamp.
    ///
    /// It is usually better to use the less error-prone
    /// `webpki::Time::try_from(time: &std::time::SystemTime)` instead when
    /// `std::time::SystemTime` is available (when `#![no_std]` isn't being
    /// used).
    pub fn from_seconds_since_unix_epoch(secs: u64) -> Time { Time(secs) }
}
