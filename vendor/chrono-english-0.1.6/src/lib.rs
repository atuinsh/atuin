//! ## Parsing English Dates
//!
//! I've always admired the ability of the GNU `date` command to
//! convert "English" expressions to dates and times with `date -d expr`.
//! `chrono-english` does similar expressions, although with extensions, so
//! that for instance you can specify both the day and the time "next friday 8pm".
//! No attempt at full natural language parsing is made - only a limited set of
//! patterns is supported.
//!
//! ## Supported Formats
//!
//! `chrono-english` does _absolute_ dates:  ISO-like dates "2018-04-01" and the month name forms
//! "1 April 2018" and "April 1, 2018". (There's no ambiguity so both of these forms are fine)
//!
//! The informal "01/04/18" or American form "04/01/18" is supported.
//! There is a `Dialect` enum to specify what kind of date English you would like to speak.
//! Both short and long years are accepted in this form; short dates pivot between 1940 and 2040.
//!
//! Then there are are _relative_ dates like 'April 1' and '9/11' (this
//! if using `Dialect::Us`). The current year is assumed, but this can be modified by 'next'
//! and 'last'. For instance, it is now the 13th of March, 2018: 'April 1' and 'next April 1'
//! are in 2018; 'last April 1' is in 2017.
//!
//! Another relative form is simply a month name
//! like 'apr' or 'April' (case-insensitive, only first three letters significant) where the
//! day is assumed to be the 1st.
//!
//! A week-day works in the same way: 'friday' means this
//! coming Friday, relative to today. 'last Friday' is unambiguous,
//! but 'next Friday' has different meanings; in the US it means the same as 'Friday'
//! but otherwise it means the Friday of next week (plus 7 days)
//!
//! Date and time can be specified also by a number of time units. So "2 days", "3 hours".
//! Again, first three letters, but 'd','m' and 'y' are understood (so "3h"). We make
//! a distinction between _second_ intervals (seconds,minutes,hours,days,weeks) and _month_
//! intervals (months,years).  Month intervals always give us the same date, if possible
//! But adding a month to "30 Jan" will give "28 Feb" or "29 Feb" depending if a leap year.
//!
//! Finally, dates may be followed by time. Either 'formal' like 18:03, with optional
//! second (like 18:03:40) or 'informal' like 6.03pm. So one gets "next friday 8pm' and so
//! forth.
//!
//! ## API
//!
//! There is exactly one entry point, which is given the date string, a `DateTime` from
//! which relative dates and times operate, and a dialect (either `Dialect::Uk`
//! or `Dialect::Us` currently.) The base time also specifies the desired timezone.
//!
//! ```ignore
//! extern crate chrono_english;
//! extern crate chrono;
//! use chrono_english::{parse_date_string,Dialect};
//!
//! use chrono::prelude::*;
//!
//! let date_time = parse_date_string("next friday 8pm", Local::now(), Dialect::Uk)?;
//! println!("{}",date_time.format("%c"));
//! ```
//!
//! There is a little command-line program `parse-date` in the `examples` folder which can be used to play
//! with these expressions.
//!
//!

extern crate scanlex;
extern crate time;
extern crate chrono;
use chrono::prelude::*;

mod parser;
mod errors;
mod types;
use types::*;
use errors::*;

pub use errors::{DateResult,DateError};

#[derive(Clone,Copy)]
pub enum Dialect {
    Uk,
    Us
}

pub fn parse_date_string<Tz: TimeZone>(s: &str, now: DateTime<Tz>, dialect: Dialect) -> DateResult<DateTime<Tz>>
where Tz::Offset: Copy {
    let mut dp = parser::DateParser::new(s);
    if let Dialect::Us = dialect {
        dp = dp.american_date();
    }
    let d = dp.parse()?;

    // we may have explicit hour:minute:sec
    let tspec = match d.time {
        Some(tspec) => tspec,
        None => TimeSpec::new_empty(),
    };
    if tspec.offset.is_some() {
     //   return DateTime::fix()::parse_from_rfc3339(s);
    }
    let date_time = if let Some(dspec) = d.date {
        dspec.to_date_time(now,tspec,dp.american).or_err("bad date")?
    } else { // no date, time set for today's date
        tspec.to_date_time(now.date()).or_err("bad time")?
    };
    Ok(date_time)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FMT_ISO: &str = "%+";

    fn display(t: DateResult<DateTime<Utc>>) -> String {
        t.unwrap().format(FMT_ISO).to_string()
    }

    #[test]
    fn basics() {
        let base = parse_date_string("2018-03-21 11:00",Utc::now(),Dialect::Uk).unwrap();

        // Day of week - relative to today. May have a time part
        assert_eq!(display(parse_date_string("friday",base,Dialect::Uk)),"2018-03-23T00:00:00+00:00");
        assert_eq!(display(parse_date_string("friday 10:30",base,Dialect::Uk)),"2018-03-23T10:30:00+00:00");
        assert_eq!(display(parse_date_string("friday 8pm",base,Dialect::Uk)),"2018-03-23T20:00:00+00:00");

        // The day of week is the _next_ day after today, so "Tuesday" is the next Tuesday after Wednesday
        assert_eq!(display(parse_date_string("tues",base,Dialect::Uk)),"2018-03-27T00:00:00+00:00");

        // The expression 'next Monday' is ambiguous; in the US it means the day following (same as 'Monday')
        // (This is how the `date` command interprets it)
        assert_eq!(display(parse_date_string("next mon",base,Dialect::Us)),"2018-03-26T00:00:00+00:00");
        // but otherwise it means the day in the next week..
        assert_eq!(display(parse_date_string("next mon",base,Dialect::Uk)),"2018-04-02T00:00:00+00:00");

        assert_eq!(display(parse_date_string("last fri 9.30",base,Dialect::Uk)),"2018-03-16T09:30:00+00:00");

        // date expressed as month, day - relative to today. May have a time part
        assert_eq!(display(parse_date_string("9/11",base,Dialect::Us)),"2018-09-11T00:00:00+00:00");
        assert_eq!(display(parse_date_string("last 9/11",base,Dialect::Us)),"2017-09-11T00:00:00+00:00");
        assert_eq!(display(parse_date_string("last 9/11 9am",base,Dialect::Us)),"2017-09-11T09:00:00+00:00");
        assert_eq!(display(parse_date_string("April 1 8.30pm",base,Dialect::Uk)),"2018-04-01T20:30:00+00:00");

        // advance by time unit from today
        // without explicit time, use base time - otherwise override
        assert_eq!(display(parse_date_string("2d",base,Dialect::Uk)),"2018-03-23T11:00:00+00:00");
        assert_eq!(display(parse_date_string("2d 03:00",base,Dialect::Uk)),"2018-03-23T03:00:00+00:00");
        assert_eq!(display(parse_date_string("3 weeks",base,Dialect::Uk)),"2018-04-11T11:00:00+00:00");
        assert_eq!(display(parse_date_string("3h",base,Dialect::Uk)),"2018-03-21T14:00:00+00:00");
        assert_eq!(display(parse_date_string("6 months",base,Dialect::Uk)),"2018-09-21T00:00:00+00:00");
        assert_eq!(display(parse_date_string("6 months ago",base,Dialect::Uk)),"2017-09-21T00:00:00+00:00");
        assert_eq!(display(parse_date_string("3 hours ago",base,Dialect::Uk)),"2018-03-21T08:00:00+00:00");
        assert_eq!(display(parse_date_string(" -3h",base,Dialect::Uk)),"2018-03-21T08:00:00+00:00");
        assert_eq!(display(parse_date_string(" -3 month",base,Dialect::Uk)),"2017-12-21T00:00:00+00:00");

        // absolute date with year, month, day - formal ISO and informal UK or US
        assert_eq!(display(parse_date_string("2017-06-30",base,Dialect::Uk)),"2017-06-30T00:00:00+00:00");
        assert_eq!(display(parse_date_string("30/06/17",base,Dialect::Uk)),"2017-06-30T00:00:00+00:00");
        assert_eq!(display(parse_date_string("06/30/17",base,Dialect::Us)),"2017-06-30T00:00:00+00:00");

        // may be followed by time part, formal and informal
        assert_eq!(display(parse_date_string("2017-06-30 08:20:30",base,Dialect::Uk)),"2017-06-30T08:20:30+00:00");
        assert_eq!(display(parse_date_string("2017-06-30 08:20:30 +02:00",base,Dialect::Uk)),"2017-06-30T06:20:30+00:00");
        assert_eq!(display(parse_date_string("2017-06-30 08:20:30 +0200",base,Dialect::Uk)),"2017-06-30T06:20:30+00:00");
        assert_eq!(display(parse_date_string("2017-06-30T08:20:30Z",base,Dialect::Uk)),"2017-06-30T08:20:30+00:00");
        assert_eq!(display(parse_date_string("2017-06-30T08:20:30",base,Dialect::Uk)),"2017-06-30T08:20:30+00:00");
        assert_eq!(display(parse_date_string("2017-06-30 8.20",base,Dialect::Uk)),"2017-06-30T08:20:00+00:00");
        assert_eq!(display(parse_date_string("2017-06-30 8.30pm",base,Dialect::Uk)),"2017-06-30T20:30:00+00:00");
        assert_eq!(display(parse_date_string("2017-06-30 8:30pm",base,Dialect::Uk)),"2017-06-30T20:30:00+00:00");
        assert_eq!(display(parse_date_string("2017-06-30 2am",base,Dialect::Uk)),"2017-06-30T02:00:00+00:00");
        assert_eq!(display(parse_date_string("30 June 2018",base,Dialect::Uk)),"2018-06-30T00:00:00+00:00");
        assert_eq!(display(parse_date_string("June 30, 2018",base,Dialect::Uk)),"2018-06-30T00:00:00+00:00");
        assert_eq!(display(parse_date_string("June   30,    2018",base,Dialect::Uk)),"2018-06-30T00:00:00+00:00");


    }

}
