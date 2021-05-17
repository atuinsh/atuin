# Parsing English Dates

I've always admired the ability of the GNU `date` command to
convert "English" expressions to dates and times with `date -d expr`.
`chrono-english` does similar expressions, although with extensions, so
that for instance you can specify both the day and the time "next friday 8pm".
No attempt at full natural language parsing is made - only a limited set of
patterns is supported.

## Supported Formats

`chrono-english` does _absolute_ dates:  ISO-like dates "2018-04-01" and the month name forms
"1 April 2018" and "April 1, 2018". (There's no ambiguity so both of these forms are fine)

The informal "01/04/18" or American form "04/01/18" is supported.
There is a `Dialect` enum to specify what kind of date English you would like to speak.
Both short and long years are accepted in this form; short dates pivot between 1940 and 2040.

Then there are are _relative_ dates like 'April 1' and '9/11' (this
if using `Dialect::Us`). The current year is assumed, but this can be modified by 'next'
and 'last'. For instance, it is now the 13th of March, 2018: 'April 1' and 'next April 1'
are in 2018; 'last April 1' is in 2017.

Another relative form is simply a month name
like 'apr' or 'April' (case-insensitive, only first three letters significant) where the
day is assumed to be the 1st.

A week-day works in the same way: 'friday' means this
coming Friday, relative to today. 'last Friday' is unambiguous,
but 'next Friday' has different meanings; in the US it means the same as 'Friday'
but otherwise it means the Friday of next week (plus 7 days)

Date and time can be specified also by a number of time units. So "2 days", "3 hours".
Again, first three letters, but 'd','m' and 'y' are understood (so "3h"). We make
a distinction between _second_ intervals (seconds,minutes,hours), _day_ intervals (days,weeks)
 and _month_ intervals (months,years).

Second intervals are not followed by a time, but day and month intervals can be. Without
a time, a day interval has the same time as the base time (which defaults to 'now')

Month intervals always give us the same date, if possible
But adding a month to "30 Jan" will give "28 Feb" or "29 Feb" depending if a leap year.

Finally, dates may be followed by time. Either 'formal' like 18:03, with optional
second (like 18:03:40) or 'informal' like 6.03pm. So one gets "next friday 8pm' and so
forth.

## API

There is exactly one entry point, which is given the date string, a `DateTime` from
which relative dates and times operate, and a dialect (either `Dialect::Uk`
or `Dialect::Us` currently.) The base time also specifies the desired timezone.

```rust
extern crate chrono_english;
use chrono_english::{parse_date_string,Dialect};

extern crate chrono;
use chrono::prelude::*;

let date_time = parse_date_string("next friday 8pm", Local::now(), Dialect::Uk)?;
println!("{}",date_time.format("%c"));
```
There is a little command-line program `parse-date` in `examples` which can be used to play
with these expressions:

```
$ alias p='cargo run --quiet --example parse-date --'
$ p 'next April'
base Wed Mar 14 20:10:37 2018 +0200
calc Sun Apr  1 00:00:00 2018 +0200
$ p '20/03/18 12:04'
base Wed Mar 14 20:12:44 2018 +0200
calc Tue Mar 20 12:04:00 2018 +0200
$ p '9/11/01' --american
base Wed Mar 14 20:13:08 2018 +0200
calc Tue Sep 11 00:00:00 2001 +0200
$ p 'next fri 8pm' '2018-03-14'
base Wed Mar 14 00:00:00 2018 +0200
calc Fri Mar 16 20:00:00 2018 +0200
```



