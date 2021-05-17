extern crate chrono_english;
use chrono_english::{parse_date_string,Dialect};

extern crate chrono;
use chrono::prelude::*;

extern crate lapp;

use std::fmt::Display;
use std::error::Error;
type BoxResult<T> = Result<T,Box<dyn Error>>;

const USAGE: &str = "
Parsing Dates in English
  -a, --american informal dates are like 9/11 not 20/03
  -u, --utc  evaluate in UTC, not Local timezone
  <date> (string) the date
  <base> (default now) the base for relative dates
";

const FMT_C: &str = "%c %z";
const FMT_ISO: &str = "%+";

fn parse_and_compare<Tz: TimeZone>(datestr: &str, basestr: &str, now: DateTime<Tz>, dialect: Dialect) -> BoxResult<()>
where Tz::Offset: Display, Tz::Offset: Copy {
    let def = basestr == "now";
    let base = parse_date_string(basestr, now, dialect)?;
    let date_time = parse_date_string(&datestr, base, dialect)?;
    if ! def {
        println!("base {} ({})", base.format(FMT_C), base.format(FMT_ISO));
    }
    println!("calc {} ({})", date_time.format(FMT_C), date_time.format(FMT_ISO));
    Ok(())
}

fn run() -> BoxResult<()> {
    let args = lapp::parse_args(USAGE);
    let utc = args.get_bool("utc");
    let datestr = args.get_string("date");
    let basestr = args.get_string("base");
    let dialect = if args.get_bool("american") {
        Dialect::Us
    } else {
        Dialect::Uk
    };
    if utc {
        parse_and_compare(&datestr,&basestr, Utc::now(), dialect)?;
    } else {
        parse_and_compare(&datestr,&basestr, Local::now(), dialect)?;
    }

    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {}",e);
        std::process::exit(1);
    }
}
