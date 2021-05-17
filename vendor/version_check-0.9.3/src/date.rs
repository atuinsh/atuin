use std::fmt;

/// Release date including year, month, and day.
// Internal storage is: y[31..9] | m[8..5] | d[5...0].
#[derive(Debug, PartialEq, Eq, Copy, Clone, PartialOrd, Ord)]
pub struct Date(u32);

impl Date {
    /// Reads the release date of the running compiler. If it cannot be
    /// determined (see the [top-level documentation](crate)), returns `None`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use version_check::Date;
    ///
    /// match Date::read() {
    ///     Some(d) => format!("The release date is: {}", d),
    ///     None => format!("Failed to read the release date.")
    /// };
    /// ```
    pub fn read() -> Option<Date> {
        ::get_version_and_date()
            .and_then(|(_, date)| date)
            .and_then(|date| Date::parse(&date))
    }

    /// Return the original (YYYY, MM, DD).
    fn to_ymd(&self) -> (u16, u8, u8) {
        let y = self.0 >> 9;
        let m = (self.0 << 23) >> 28;
        let d = (self.0 << 27) >> 27;
        (y as u16, m as u8, d as u8)
    }

    /// Parse a release date of the form `%Y-%m-%d`. Returns `None` if `date` is
    /// not in `%Y-%m-%d` format.
    ///
    /// # Example
    ///
    /// ```rust
    /// use version_check::Date;
    ///
    /// let date = Date::parse("2016-04-20").unwrap();
    ///
    /// assert!(date.at_least("2016-01-10"));
    /// assert!(date.at_most("2016-04-20"));
    /// assert!(date.exactly("2016-04-20"));
    ///
    /// assert!(Date::parse("March 13, 2018").is_none());
    /// assert!(Date::parse("1-2-3-4-5").is_none());
    /// ```
    pub fn parse(date: &str) -> Option<Date> {
        let ymd: Vec<u32> = date.split("-")
            .filter_map(|s| s.parse::<u32>().ok())
            .collect();

        if ymd.len() != 3 {
            return None
        }

        let (y, m, d) = (ymd[0], ymd[1], ymd[2]);
        Some(Date((y << 9) | ((m & 0xF) << 5) | (d & 0x1F)))
    }

    /// Returns `true` if `self` occurs on or after `date`.
    ///
    /// If `date` occurs before `self`, or if `date` is not in `%Y-%m-%d`
    /// format, returns `false`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use version_check::Date;
    ///
    /// let date = Date::parse("2020-01-01").unwrap();
    ///
    /// assert!(date.at_least("2019-12-31"));
    /// assert!(date.at_least("2020-01-01"));
    /// assert!(date.at_least("2014-04-31"));
    ///
    /// assert!(!date.at_least("2020-01-02"));
    /// assert!(!date.at_least("2024-08-18"));
    /// ```
    pub fn at_least(&self, date: &str) -> bool {
        Date::parse(date)
            .map(|date| self >= &date)
            .unwrap_or(false)
    }

    /// Returns `true` if `self` occurs on or before `date`.
    ///
    /// If `date` occurs after `self`, or if `date` is not in `%Y-%m-%d`
    /// format, returns `false`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use version_check::Date;
    ///
    /// let date = Date::parse("2020-01-01").unwrap();
    ///
    /// assert!(date.at_most("2020-01-01"));
    /// assert!(date.at_most("2020-01-02"));
    /// assert!(date.at_most("2024-08-18"));
    ///
    /// assert!(!date.at_most("2019-12-31"));
    /// assert!(!date.at_most("2014-04-31"));
    /// ```
    pub fn at_most(&self, date: &str) -> bool {
        Date::parse(date)
            .map(|date| self <= &date)
            .unwrap_or(false)
    }

    /// Returns `true` if `self` occurs exactly on `date`.
    ///
    /// If `date` is not exactly `self`, or if `date` is not in `%Y-%m-%d`
    /// format, returns `false`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use version_check::Date;
    ///
    /// let date = Date::parse("2020-01-01").unwrap();
    ///
    /// assert!(date.exactly("2020-01-01"));
    ///
    /// assert!(!date.exactly("2019-12-31"));
    /// assert!(!date.exactly("2014-04-31"));
    /// assert!(!date.exactly("2020-01-02"));
    /// assert!(!date.exactly("2024-08-18"));
    /// ```
    pub fn exactly(&self, date: &str) -> bool {
        Date::parse(date)
            .map(|date| self == &date)
            .unwrap_or(false)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let (y, m, d) = self.to_ymd();
        write!(f, "{}-{:02}-{:02}", y, m, d)
    }
}

#[cfg(test)]
mod tests {
    use super::Date;

    macro_rules! reflexive_display {
        ($string:expr) => (
            assert_eq!(Date::parse($string).unwrap().to_string(), $string);
        )
    }

    #[test]
    fn display() {
        reflexive_display!("2019-05-08");
        reflexive_display!("2000-01-01");
        reflexive_display!("2000-12-31");
        reflexive_display!("2090-12-31");
        reflexive_display!("1999-02-19");
    }
}
