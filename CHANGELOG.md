# Changelog

All notable changes to this project will be documented in this file.

## [17.1.0] - 2023-12-10

### Bug Fixes

- Initial list of history in workspace mode ([#1356](https://github.com/atuinsh/atuin/issues/1356))
- Add Appkit to the package build ([#1358](https://github.com/atuinsh/atuin/issues/1358))
- Bind in the most popular modes ([#1360](https://github.com/atuinsh/atuin/issues/1360))
- Only trigger up-arrow on first line ([#1359](https://github.com/atuinsh/atuin/issues/1359))
- Clean up the fish script options ([#1370](https://github.com/atuinsh/atuin/issues/1370))
- Use fish builtins for `enter_accept` ([#1373](https://github.com/atuinsh/atuin/issues/1373))
- Make `atuin account delete` void session + key ([#1393](https://github.com/atuinsh/atuin/issues/1393))
- New clippy lints ([#1395](https://github.com/atuinsh/atuin/issues/1395))
- Accept multiline commands ([#1418](https://github.com/atuinsh/atuin/issues/1418))
- Reenable enter_accept for bash ([#1408](https://github.com/atuinsh/atuin/issues/1408))
- Respect ZSH's $ZDOTDIR environment variable ([#942](https://github.com/atuinsh/atuin/issues/942))

### Documentation

- Update sync.md ([#1409](https://github.com/atuinsh/atuin/issues/1409))
- Update Arch Linux package URL in advanced-install.md ([#1407](https://github.com/atuinsh/atuin/issues/1407))
- New stats config ([#1412](https://github.com/atuinsh/atuin/issues/1412))

### Features

- Add a nixpkgs overlay ([#1357](https://github.com/atuinsh/atuin/issues/1357))
- Add metrics server and http metrics ([#1394](https://github.com/atuinsh/atuin/issues/1394))
- Add some metrics related to Atuin as an app ([#1399](https://github.com/atuinsh/atuin/issues/1399))
- Allow configuring stats prefix ([#1411](https://github.com/atuinsh/atuin/issues/1411))
- Allow spaces in stats prefixes ([#1414](https://github.com/atuinsh/atuin/issues/1414))

### Miscellaneous Tasks

- Update to sqlx 0.7.3 ([#1416](https://github.com/atuinsh/atuin/issues/1416))
- `cargo update` ([#1419](https://github.com/atuinsh/atuin/issues/1419))
- Update rusty_paseto and rusty_paserk ([#1420](https://github.com/atuinsh/atuin/issues/1420))
- Run dependabot weekly, not daily ([#1423](https://github.com/atuinsh/atuin/issues/1423))
- Don't group deps ([#1424](https://github.com/atuinsh/atuin/issues/1424))
- Add contributor image to README ([#1430](https://github.com/atuinsh/atuin/issues/1430))
- Setup git cliff ([#1431](https://github.com/atuinsh/atuin/issues/1431))

## [17.0.1] - 2023-10-28

### Bug Fixes

- Improve output for `enter_accept` ([#1341](https://github.com/atuinsh/atuin/issues/1341))
- Improve output of `enter_accept` ([#1342](https://github.com/atuinsh/atuin/issues/1342))
- Clear old cmd snippet ([#1350](https://github.com/atuinsh/atuin/issues/1350))

## [17.0.0] - 2023-10-26

### Bug Fixes

- Detect non amd64 ubuntu and handle ([#1131](https://github.com/atuinsh/atuin/issues/1131))
- Workspace Filtermode not handled in skim engine ([#1273](https://github.com/atuinsh/atuin/issues/1273))
- Ignore stderr messages ([#1320](https://github.com/atuinsh/atuin/issues/1320))
- Disable the up-arrow keybinding for Nushell ([#1329](https://github.com/atuinsh/atuin/issues/1329))

### Documentation

- Update `workspace` config key to `workspaces` ([#1174](https://github.com/atuinsh/atuin/issues/1174))
- Document the available format options of History list command ([#1234](https://github.com/atuinsh/atuin/issues/1234))

### Features

- Mouse selection support ([#1209](https://github.com/atuinsh/atuin/issues/1209))
- Configure SearchMode for KeyUp invocation #1216 ([#1224](https://github.com/atuinsh/atuin/issues/1224))
- Try installing via paru for the AUR ([#1262](https://github.com/atuinsh/atuin/issues/1262))
- Copy to clipboard ([#1249](https://github.com/atuinsh/atuin/issues/1249))

### Refactor

- Duplications reduced in order to align implementations of reading history files ([#1247](https://github.com/atuinsh/atuin/issues/1247))

### Config.md

- Invert mode detailed options ([#1225](https://github.com/atuinsh/atuin/issues/1225))

## [16.0.0] - 2023-08-07

### Bug Fixes

- Adjust broken link to supported shells ([#1013](https://github.com/atuinsh/atuin/issues/1013))
- Fixes unix specific impl of shutdown_signal ([#1061](https://github.com/atuinsh/atuin/issues/1061))
- Teapot is a cup of coffee ([#1137](https://github.com/atuinsh/atuin/issues/1137))
- Nushell empty hooks ([#1138](https://github.com/atuinsh/atuin/issues/1138))
- List all presently documented commands ([#1140](https://github.com/atuinsh/atuin/issues/1140))
- Correct command overview paths ([#1145](https://github.com/atuinsh/atuin/issues/1145))

### Features

- Do not allow empty passwords durring account creation ([#1029](https://github.com/atuinsh/atuin/issues/1029))

### Skim

- Fix filtering aggregates ([#1114](https://github.com/atuinsh/atuin/issues/1114))

## [15.0.0] - 2023-05-28

### Documentation

- Fix broken links in README.md ([#920](https://github.com/atuinsh/atuin/issues/920))
- Fix "From source" `cd` command ([#937](https://github.com/atuinsh/atuin/issues/937))

### Features

- Add delete account option (attempt 2) ([#980](https://github.com/atuinsh/atuin/issues/980))

### Miscellaneous Tasks

- Uuhhhhhh crypto lol ([#805](https://github.com/atuinsh/atuin/issues/805))
- Fix participle "be ran" -> "be run" ([#939](https://github.com/atuinsh/atuin/issues/939))

### Cwd_filter

- Much like history_filter, only it applies to cwd ([#904](https://github.com/atuinsh/atuin/issues/904))

## [14.0.0] - 2023-04-01

### Bug Fixes

- Always read session_path from settings ([#757](https://github.com/atuinsh/atuin/issues/757))
- Use case-insensitive comparison ([#776](https://github.com/atuinsh/atuin/issues/776))
- Many wins were broken :memo: ([#789](https://github.com/atuinsh/atuin/issues/789))
- Paste into terminal after switching modes ([#793](https://github.com/atuinsh/atuin/issues/793))
- Record negative exit codes ([#821](https://github.com/atuinsh/atuin/issues/821))
- Allow nix package to fetch dependencies from git ([#832](https://github.com/atuinsh/atuin/issues/832))

### Documentation

- Fix activity graph link ([#753](https://github.com/atuinsh/atuin/issues/753))

### Features

- Add common default keybindings ([#719](https://github.com/atuinsh/atuin/issues/719))
- Add an inline view mode ([#648](https://github.com/atuinsh/atuin/issues/648))
- Add *Nushell* support ([#788](https://github.com/atuinsh/atuin/issues/788))
- Add github action to test the nix builds ([#833](https://github.com/atuinsh/atuin/issues/833))

### Miscellaneous Tasks

- Remove tui vendoring ([#804](https://github.com/atuinsh/atuin/issues/804))
- Use fork of skim ([#803](https://github.com/atuinsh/atuin/issues/803))

### Nix

- Add flake-compat ([#743](https://github.com/atuinsh/atuin/issues/743))

## [13.0.0] - 2023-02-26

### Documentation

- Remove human short flag from docs, duplicate of help -h ([#663](https://github.com/atuinsh/atuin/issues/663))
- Fix typo in zh-CN/README.md ([#666](https://github.com/atuinsh/atuin/issues/666))
- Add static activity graph example ([#680](https://github.com/atuinsh/atuin/issues/680))

### Features

- Add new flag to allow custom output format ([#662](https://github.com/atuinsh/atuin/issues/662))

### Fish

- Fix `atuin init` for the fish shell ([#699](https://github.com/atuinsh/atuin/issues/699))

### Install.sh

- Fallback to using cargo ([#639](https://github.com/atuinsh/atuin/issues/639))

## [12.0.0] - 2022-11-06

### Documentation

- Add more details about date parsing in the stats command ([#579](https://github.com/atuinsh/atuin/issues/579))

## [0.10.0] - 2022-06-06

### Miscellaneous Tasks

- Allow specifiying the limited of returned entries ([#364](https://github.com/atuinsh/atuin/issues/364))

## [0.9.0] - 2022-04-23

### README

- Add MacPorts installation instructions ([#302](https://github.com/atuinsh/atuin/issues/302))

## [0.8.1] - 2022-04-12

### Bug Fixes

- Get install.sh working on UbuntuWSL ([#260](https://github.com/atuinsh/atuin/issues/260))

## [0.8.0] - 2021-12-17

### Bug Fixes

- Resolve some issues with install.sh ([#188](https://github.com/atuinsh/atuin/issues/188))

### Features

- Login/register no longer blocking ([#216](https://github.com/atuinsh/atuin/issues/216))

## [0.7.2] - 2021-12-08

### Bug Fixes

- Dockerfile with correct glibc ([#198](https://github.com/atuinsh/atuin/issues/198))

### Features

- Allow input of credentials from stdin ([#185](https://github.com/atuinsh/atuin/issues/185))

### Miscellaneous Tasks

- Some new linting ([#201](https://github.com/atuinsh/atuin/issues/201))
- Supply pre-build docker image ([#199](https://github.com/atuinsh/atuin/issues/199))
- Add more eyre contexts ([#200](https://github.com/atuinsh/atuin/issues/200))
- Improve build times ([#213](https://github.com/atuinsh/atuin/issues/213))

## [0.7.1] - 2021-05-10

### Features

- Build individual crates ([#109](https://github.com/atuinsh/atuin/issues/109))

## [0.6.3] - 2021-04-26

### Bug Fixes

- Help text

### Features

- Use directories project data dir

### Miscellaneous Tasks

- Use structopt wrapper instead of building clap by hand

<!-- generated by git-cliff -->
