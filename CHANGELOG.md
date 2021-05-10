# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project (mostly) adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
