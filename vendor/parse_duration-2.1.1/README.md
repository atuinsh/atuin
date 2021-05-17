# parse_duration
[![Crates.io](https://img.shields.io/crates/v/parse_duration.svg)](https://crates.io/crates/parse_duration)
[![Travis](https://img.shields.io/travis/zeta12ti/parse_duration.svg)](https://travis-ci.org/zeta12ti/parse_duration)

***IMPORTANT***: This repository is no longer being updated. Before deciding to use it, check if any of the [issues](https://github.com/zeta12ti/parse_duration/issues?q=is%3Aissue+is%3Aopen+sort%3Aupdated-desc) are deal breakers. In particular, this crate should not be used with untrusted input (see [this issue](https://github.com/zeta12ti/parse_duration/issues/21)).

This crate provides a function `parse` for parsing strings into durations.
The parser is based on the standard set by
[systemd.time](https://www.freedesktop.org/software/systemd/man/systemd.time.html#Parsing%20Time%20Spans),
but extends it significantly.
For example, negative numbers, decimals and exponents are allowed.

```
extern crate parse_duration;

use parse_duration::parse;
use std::time::Duration;

// One hour less than a day
assert_eq!(parse("1 day -1 hour"), Ok(Duration::new(82_800, 0)));
// Using exponents
assert_eq!(parse("1.26e-1 days"), Ok(Duration::new(10_886, 400_000_000)));
// Extra things will be ignored
assert_eq!(
    parse("Duration: 1 hour, 15 minutes and 29 seconds"),
    Ok(Duration::new(4529, 0))
);
```


## Documentation
Documentation may be found [on docs.rs](https://docs.rs/parse_duration).


## Minimum Rust version policy
This crate's minimum supported rustc version is 1.28.0.

If the minimum rustc version needs to be increased, there will be a new major version. For example, if parse\_duration 2.0.0 requires rustc 1.28.0, then parse\_duration 2.x.y will also only require rustc 1.28.0. Since this crate is fairly simple, there likely won't be any need to increase the minimum version in the foreseeable future.

## License
This software is licensed under the MIT License.


## Contributing
Feel free to file an issue or submit a pull request if there's a bug you want fixed
or a feature you want implemented.

By contributing to this project, you agree to license your code under the terms of
the MIT License.
