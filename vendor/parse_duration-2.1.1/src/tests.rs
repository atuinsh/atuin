// MIT License
//
// Copyright (c) 2017 The parse_duration Developers
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use super::parse;
use num::BigInt;
use std::time::Duration;

macro_rules! test_parse {
    (fn $fun:ident($string: expr, $seconds: expr, $nanoseconds: expr)) => {
        #[test]
        fn $fun() {
            assert_eq!(parse($string), Ok(Duration::new($seconds, $nanoseconds)))
        }
    };
}

macro_rules! test_invalid {
    (fn $fun:ident($string: expr, $error: expr)) => {
        #[test]
        fn $fun() {
            assert_eq!(parse($string), Err($error));
        }
    };
}

test_parse!(fn nano1("1nsec", 0, 1));
test_parse!(fn nano2("1ns", 0, 1));
test_parse!(fn nano_dec("1.07 ns", 0, 1));
test_parse!(fn nano_exp1("1.07e5 ns", 0, 107_000));
test_parse!(fn nano_exp2("1.07e+5 ns", 0, 107_000));
test_parse!(fn nano_exp3("1.07e-5 ns", 0, 0));
test_parse!(fn nano_exp4("1e5 ns", 0, 100_000));
test_parse!(fn nano_exp5("1e+5 ns", 0, 100_000));
test_parse!(fn nano_exp6("1e-5 ns", 0, 0));

test_parse!(fn micro1("1usec", 0, 1_000));
test_parse!(fn micro2("1us", 0, 1_000));
test_parse!(fn micro_dec("1.07 us", 0, 1_070));
test_parse!(fn micro_exp1("1.07e5 us", 0, 107_000_000));
test_parse!(fn micro_exp2("1.07e+5 us", 0, 107_000_000));
test_parse!(fn micro_exp3("1.07e-5 us", 0, 0));
test_parse!(fn micro_exp4("1e5 us", 0, 100_000_000));
test_parse!(fn micro_exp5("1e+5 us", 0, 100_000_000));
test_parse!(fn micro_exp6("1e-5 us", 0, 0));

test_parse!(fn milli1("1msec", 0, 1_000_000));
test_parse!(fn milli2("1ms", 0, 1_000_000));
test_parse!(fn milli_dec("1.07 ms", 0, 1_070_000));
test_parse!(fn milli_exp1("1.07e5 ms", 107, 0));
test_parse!(fn milli_exp2("1.07e+5 ms", 107, 0));
test_parse!(fn milli_exp3("1.07e-5 ms", 0, 10));
test_parse!(fn milli_exp4("1e5 ms", 100, 0));
test_parse!(fn milli_exp5("1e+5 ms", 100, 0));
test_parse!(fn milli_exp6("1e-5 ms", 0, 10));

test_parse!(fn sec1("1seconds", 1, 0));
test_parse!(fn sec2("1second", 1, 0));
test_parse!(fn sec3("1sec", 1, 0));
test_parse!(fn sec4("1s", 1, 0));
test_parse!(fn sec_dec("1.07 s", 1, 70_000_000));
test_parse!(fn sec_exp1("1.07e5 s", 107_000, 0));
test_parse!(fn sec_exp2("1.07e+5 s", 107_000, 0));
test_parse!(fn sec_exp3("1.07e-5 s", 0, 10_700));
test_parse!(fn sec_exp4("1e5 s", 100_000, 0));
test_parse!(fn sec_exp5("1e+5 s", 100_000, 0));
test_parse!(fn sec_exp6("1e-5 s", 0, 10_000));

test_parse!(fn min1("1minutes", 60, 0));
test_parse!(fn min2("1minute", 60, 0));
test_parse!(fn min3("1min", 60, 0));
test_parse!(fn min4("1m", 60, 0));
test_parse!(fn min_dec("1.07 m", 64, 200_000_000));
test_parse!(fn min_exp1("1.07e5 m", 6_420_000, 0));
test_parse!(fn min_exp2("1.07e+5 m", 6_420_000, 0));
test_parse!(fn min_exp3("1.07e-5 m", 0, 642_000));
test_parse!(fn min_exp4("1e5 m", 6_000_000, 0));
test_parse!(fn min_exp5("1e+5 m", 6_000_000, 0));
test_parse!(fn min_exp6("1e-5 m", 0, 600_000));

test_parse!(fn hour1("1hours", 3_600, 0));
test_parse!(fn hour2("1hour", 3_600, 0));
test_parse!(fn hour3("1hr", 3_600, 0));
test_parse!(fn hour4("1h", 3_600, 0));
test_parse!(fn hour_dec("1.07 h", 3_852, 0));
test_parse!(fn hour_exp1("1.07e5 h", 385_200_000, 0));
test_parse!(fn hour_exp2("1.07e+5 h", 385_200_000, 0));
test_parse!(fn hour_exp3("1.07e-5 h", 0, 38_520_000));
test_parse!(fn hour_exp4("1e5 h", 360_000_000, 0));
test_parse!(fn hour_exp5("1e+5 h", 360_000_000, 0));
test_parse!(fn hour_exp6("1e-5 h", 0, 36_000_000));

test_parse!(fn day1("1days", 86_400, 0));
test_parse!(fn day2("1day", 86_400, 0));
test_parse!(fn day3("1d", 86_400, 0));
test_parse!(fn day_dec("1.07 d", 92_448, 0));
test_parse!(fn day_exp1("1.07e5 d", 9_244_800_000, 0));
test_parse!(fn day_exp2("1.07e+5 d", 9_244_800_000, 0));
test_parse!(fn day_exp3("1.07e-5 d", 0, 924_480_000));
test_parse!(fn day_exp4("1e5 d", 8_640_000_000, 0));
test_parse!(fn day_exp5("1e+5 d", 8_640_000_000, 0));
test_parse!(fn day_exp6("1e-5 d", 0, 864_000_000));

test_parse!(fn week1("1weeks", 604_800, 0));
test_parse!(fn week2("1week", 604_800, 0));
test_parse!(fn week3("1w", 604_800, 0));
test_parse!(fn week_dec("1.07 w", 647_136, 0));
test_parse!(fn week_exp1("1.07e5 w", 64_713_600_000, 0));
test_parse!(fn week_exp2("1.07e+5 w", 64_713_600_000, 0));
test_parse!(fn week_exp3("1.07e-5 w", 6, 471_360_000));
test_parse!(fn week_exp4("1e5 w", 60_480_000_000, 0));
test_parse!(fn week_exp5("1e+5 w", 60_480_000_000, 0));
test_parse!(fn week_exp6("1e-5 w", 6, 48_000_000));

test_parse!(fn month1("1months", 2_629_746, 0));
test_parse!(fn month2("1month", 2_629_746, 0));
test_parse!(fn month3("1M", 2_629_746, 0));
test_parse!(fn month_dec("1.07 M", 2_813_828, 220_000_000));
test_parse!(fn month_exp1("1.07e5 M", 281_382_822_000, 0));
test_parse!(fn month_exp2("1.07e+5 M", 281_382_822_000, 0));
test_parse!(fn month_exp3("1.07e-5 M", 28, 138_282_200));
test_parse!(fn month_exp4("1e5 M", 262_974_600_000, 0));
test_parse!(fn month_exp5("1e+5 M", 262_974_600_000, 0));
test_parse!(fn month_exp6("1e-5 M", 26, 297_460_000));

test_parse!(fn year1("1years", 31_556_952, 0));
test_parse!(fn year2("1year", 31_556_952, 0));
test_parse!(fn year3("1y", 31_556_952, 0));
test_parse!(fn year_dec("1.07 y", 33_765_938, 640_000_000));
test_parse!(fn year_exp1("1.07e5 y", 3_376_593_864_000, 0));
test_parse!(fn year_exp2("1.07e+5 y", 3_376_593_864_000, 0));
test_parse!(fn year_exp3("1.07e-5 y", 337, 659_386_400));
test_parse!(fn year_exp4("1e5 y", 3_155_695_200_000, 0));
test_parse!(fn year_exp5("1e+5 y", 3_155_695_200_000, 0));
test_parse!(fn year_exp6("1e-5 y", 315, 569_520_000));

test_parse!(fn multi_with_space("1min    10 seconds", 70, 0));
test_parse!(fn multi_no_space("1min10seconds", 70, 0));
test_parse!(fn multi_out_of_order("10year1min10seconds5h", 315_587_590, 0));
test_parse!(fn multi_repetition("1min 10 minute", 660, 0));

test_parse!(fn multiple_units("16 min seconds", 960, 0));

test_parse!(fn negatives("1 day -15 minutes", 85_500, 0));
test_parse!(fn unmatched_negatives("1 day - 15 minutes", 87_300, 0));

test_parse!(fn no_unit("15", 15, 0));
test_parse!(fn no_unit_with_noise(".:++++]][][[][15[]][][]:}}}}", 15, 0));

test_invalid!(fn invalid_int("1e11232345982734592837498234 years", parse::Error::ParseInt("11232345982734592837498234".to_string())));
test_invalid!(fn invalid_unit("16 sdfwe", parse::Error::UnknownUnit("sdfwe".to_string())));
test_invalid!(fn no_value("year", parse::Error::NoValueFound("year".to_string())));
test_invalid!(fn wrong_order("year15", parse::Error::NoUnitFound("15".to_string())));

#[test]
fn number_too_big() {
    assert_eq!(
        Ok(parse("123456789012345678901234567890 seconds")),
        "123456789012345678901234567890"
            .parse::<BigInt>()
            .map(|int| Err(parse::Error::OutOfBounds(int)))
    );
}

#[test]
fn negative_duration() {
    assert_eq!(
        Ok(parse("-3 days 71 hours")),
        "-3600"
            .parse::<BigInt>()
            .map(|int| Err(parse::Error::OutOfBounds(int)))
    );
}

test_invalid!(fn not_enough_units("16 17 seconds", parse::Error::NoUnitFound("16".to_string())));
