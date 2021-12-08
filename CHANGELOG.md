# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project (mostly) adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.7.2] - 2021-12-08

6e8ec868 chore: improve build times (#213)
f2c1922e Bump itertools from 0.10.0 to 0.10.1 (#146)
e2c06052 Bump rmp-serde from 0.15.4 to 0.15.5 (#149)
d579b55d Bump rand from 0.8.3 to 0.8.4 (#152)
f539f60a chore: add more eyre contexts (#200)
2e59d6a5 Bump reqwest from 0.11.3 to 0.11.6 (#192)
e89de3f7 chore: supply pre-build docker image (#199)
07c06825 Bump tokio from 1.6.1 to 1.14.0 (#205)
46a1dab1 fix: dockerfile with correct glibc (#198)
8f91b141 chore: some new linting (#201)
27d3d81a feat: allow input of credentials from stdin (#185)
446ffb88 Resolve clippy warnings (#187)
2024884f Reordered fuzzy search (#179)
1babb41e Update README.md
0b9dc669 Add fuzzy text search mode (#142)
f0130571 Bump indicatif from 0.16.0 to 0.16.2 (#140)
cc7ce093 Bump sqlx from 0.5.2 to 0.5.5 (#139)
f8c80429 Bump tokio from 1.6.0 to 1.6.1 (#141)
802a2258 Bump tokio from 1.5.0 to 1.6.0 (#132)
4d52c5e8 Bump urlencoding from 1.3.1 to 1.3.3 (#133)
87c9f61e Bump serde from 1.0.125 to 1.0.126 (#124)
9303f482 Bump urlencoding from 1.1.1 to 1.3.1 (#125)
cb7d656c instructions to install without tap (#127)
f55d5cf0 Ignore commands beginning with a space, resolve #114 (#123)
a127408e run shellcheck (#97)
f041d7fe Adding plugin for zsh (#117)
fd90bd34 Fix doc links in sync.md (#115)
477c6852 Elementary Linux add as supported (#113)

## [0.7.1] - 2021-05-10

Very minor patch release

### Added

### Changed

### Deprecated

### Removed

### Fixed

- Fix the atuin-common build (#107)

### Security

## [0.7.0] - 2021-05-10

Thank you so much to everyone that started contributing to Atuin for this release!

- [@yuvipanda](https://github.com/yuvipanda)
- [@Sciencentistguy](https://github.com/Sciencentistguy)
- [@bl-ue](https://github.com/bl-ue)
- [@ElvishJerricco](https://github.com/ElvishJerricco)
- [@avinassh](https://github.com/avinassh)
- [@ismith](https://github.com/ismith)
- [@thedrow](https://github.com/thedrow)

And a special thank you to [@conradludgate](https://github.com/conradludgate) for his ongoing contributions :)

### Added

- Ctrl-C to exit (#53)
- Ctrl-D to exit (#65)
- Add option to not automatically bind keys (#62)
- Add importer for Resh history (#69)
- Retain the query entered if no results are found (#76)
- Support full-text querying (#75)
- Allow listing or searching with only the command as output (#89)
- Emacs-style ctrl-g, ctrl-n, ctrl-p (#77)
- `atuin logout` (#91)
- "quick access" to earlier commands via <kbd>Alt-N</kbd> (#79)

### Changed

- CI build caching (#49)
- Use an enum for dialect (#80)
- Generic importer trait (#71)
- Increased optimisation for release builds (#101)
- Shellcheck fixes for bash file (#81)
- Some general cleanup, bugfixes, and refactoring (#83, #90, #48)

### Deprecated

### Removed

### Fixed

- Ubuntu install (#46)
- Bash integration (#88)
- Newline when editing shell RC files (#60)

### Security
