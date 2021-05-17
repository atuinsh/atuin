
# 1.9.3 (February 24, 2020)
- Fix compilation regression for 1.34.0. Thanks @jlevon for reporting.

# 1.9.2 (January 11, 2020)
- Exposed `ColoredString` data through methods for purposes of interrogating the applied colours.
- Increased documentation.

# 1.9.1 (December 31, 2019)

- Remove deprecated `try!` macro in codebase
- Reduce allocations in `ColoredString` impl (PR#65)
- Added `"purple"` as match in `impl FromStr for Color` (PR#71)

# 1.9.0 (November 11, 2019)

- **[POSSIBLE_BREAKING CHANGE]:** Replace `winconsole` with `winapi`:
  - Changes `set_virtual_terminal` function signature.
- Update dependencies
- Add Dockerfile
- Respect tty discovery for CLICOLOR

# 1.8.0 (April 30, 2019)

- FEAT: support Windows 10 colors

# 1.7.0 (January, 2019)
- TECH: update lazy\_static
- FEAT: introduce respect for the `NO_COLOR` environment variable

# 1.6.1 (July 9, 2018)
- TECH: update lazy\_static
- CHORE: fix typos in README and documentation

# 1.6.0 (October 31, 2017)
- FEAT: introduced bright colors. `"hello".bright_blue().on_bright_red();`
- FEAT: introduced strikethrough styling. `"hello".strikethrough();`

# 1.5.3 (September 28, 2017)

- FEAT: derive Copy and Clone for `Color`
- FEAT: derive Clone for `ColoredString`

# 1.5.2 (July 6, 2017)

- FIX: method `Colorize::reversed` has been added. `Colorize::reverse` was a typo, that we will keep
    for compatibility

# 1.5.1 (May 9, 2017)

- Update lazy\_static to 0.2.

# 1.5.0 (May 1, 2017)

- FEAT: support for `"hello".color("blue")` (dynamic colors)

# 1.3.2 (Nov 26, 2016)

- FIX: usage of nested ColoredString again, no more style broken mid-line

# 1.3.1 (Oct 14, 2016)

- FIX: usage of ColoredString in a nested way broke the styling mid-line

# 1.3.0 (Jul 31, 2016)

- Provide various options for disabling the coloring in an API-compatible way

# 1.2.0 (Mar 30, 2016)

- Support the different formatting options, like padding and alignment

# 1.1.0 (Mar 15, 2016)

- Respect the CLICOLOR/CLICOLOR\_FORCE behavior. See [this specs](http://bixense.com/clicolors/)

# 1.0.1 (Mar 14, 2016)

- Add a CHANGLOG
- Fix crate dependencies: move `ansi_term` in dev\_dependencies

# 1.0.0 (Mar 13, 2016)

- Initial release
