// These tests are only run for the "default" test target because some of them
// can take quite a long time. Some of them take long enough that it's not
// practical to run them in debug mode. :-/

// See: https://oss-fuzz.com/testcase-detail/5673225499181056
//
// Ignored by default since it takes too long in debug mode (almost a minute).
#[test]
#[ignore]
fn fuzz1() {
    regex!(r"1}{55}{0}*{1}{55}{55}{5}*{1}{55}+{56}|;**");
}

// See: https://bugs.chromium.org/p/oss-fuzz/issues/detail?id=26505
// See: https://github.com/rust-lang/regex/issues/722
#[test]
fn empty_any_errors_no_panic() {
    assert!(regex_new!(r"\P{any}").is_err());
}
