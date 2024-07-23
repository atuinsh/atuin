# Changelog

All notable changes to this project will be documented in this file.

## [unreleased]

### Bug Fixes

- *(crate)* Add missing description ([#2106](https://github.com/atuinsh/atuin/issues/2106))
- *(crate)* Add description to daemon crate ([#2107](https://github.com/atuinsh/atuin/issues/2107))
- *(gui)* Update deps ([#2116](https://github.com/atuinsh/atuin/issues/2116))
- *(gui)* Add support for checking if the cli is installed on windows ([#2162](https://github.com/atuinsh/atuin/issues/2162))
- *(gui)* WeekInfo call on Edge ([#2252](https://github.com/atuinsh/atuin/issues/2252))
- *(gui)* Add \r for windows (shouldn't effect unix bc they should ignore it) ([#2253](https://github.com/atuinsh/atuin/issues/2253))
- *(gui)* Terminal resize overflow ([#2285](https://github.com/atuinsh/atuin/issues/2285))
- *(gui)* Kill child on block stop ([#2288](https://github.com/atuinsh/atuin/issues/2288))
- *(history)* Logic for store_failed=false ([#2284](https://github.com/atuinsh/atuin/issues/2284))
- *(themes)* Restore default theme, refactor ([#2294](https://github.com/atuinsh/atuin/issues/2294))
- *(tui)* Press ctrl-a twice should jump to beginning of line ([#2246](https://github.com/atuinsh/atuin/issues/2246))
- Cargo binstall config ([#2112](https://github.com/atuinsh/atuin/issues/2112))
- Unitless sync_frequence = 0 not parsed by humantime ([#2154](https://github.com/atuinsh/atuin/issues/2154))
- Some --help comments didn't show properly ([#2176](https://github.com/atuinsh/atuin/issues/2176))
- Ensure we cleanup all tables when deleting ([#2191](https://github.com/atuinsh/atuin/issues/2191))
- Add idx cache unique index ([#2226](https://github.com/atuinsh/atuin/issues/2226))
- Idx cache inconsistency ([#2231](https://github.com/atuinsh/atuin/issues/2231))
- Ambiguous column name ([#2232](https://github.com/atuinsh/atuin/issues/2232))


### Documentation

- *(README)* Fix broken link ([#2206](https://github.com/atuinsh/atuin/issues/2206))
- *(gui)* Update README ([#2283](https://github.com/atuinsh/atuin/issues/2283))
- Streamline readme ([#2203](https://github.com/atuinsh/atuin/issues/2203))
- Update quickstart install command ([#2205](https://github.com/atuinsh/atuin/issues/2205))


### Features

- *(bash/blesh)* Hook into BLE_ONLOAD to resolve loading order issue ([#2234](https://github.com/atuinsh/atuin/issues/2234))
- *(daemon)* Follow XDG_RUNTIME_DIR if set ([#2171](https://github.com/atuinsh/atuin/issues/2171))
- *(gui)* Automatically install and setup the cli/shell ([#2139](https://github.com/atuinsh/atuin/issues/2139))
- *(gui)* Add activity calendar to the homepage ([#2160](https://github.com/atuinsh/atuin/issues/2160))
- *(gui)* Cache zustand store in localstorage ([#2168](https://github.com/atuinsh/atuin/issues/2168))
- *(gui)* Toast with prompt for cli install, rather than auto ([#2173](https://github.com/atuinsh/atuin/issues/2173))
- *(gui)* Runbooks that run ([#2233](https://github.com/atuinsh/atuin/issues/2233))
- *(gui)* Use fancy new side nav ([#2243](https://github.com/atuinsh/atuin/issues/2243))
- *(gui)* Add runbook list, ability to create and delete, sql storage ([#2282](https://github.com/atuinsh/atuin/issues/2282))
- *(gui)* Background terminals and more ([#2303](https://github.com/atuinsh/atuin/issues/2303))
- *(gui)* Clean up home page, fix a few bugs ([#2304](https://github.com/atuinsh/atuin/issues/2304))
- *(history)* Filter out various environment variables containing potential secrets ([#2174](https://github.com/atuinsh/atuin/issues/2174))
- *(tui)* Configurable prefix character ([#2157](https://github.com/atuinsh/atuin/issues/2157))
- *(tui)* Customizable Themes ([#2236](https://github.com/atuinsh/atuin/issues/2236))
- *(tui)* Fixed preview height option ([#2286](https://github.com/atuinsh/atuin/issues/2286))
- Use cargo-dist installer from our install script ([#2108](https://github.com/atuinsh/atuin/issues/2108))
- Add user account verification ([#2190](https://github.com/atuinsh/atuin/issues/2190))
- Add GitLab PAT to secret patterns ([#2196](https://github.com/atuinsh/atuin/issues/2196))
- Add several other GitHub access token patterns ([#2200](https://github.com/atuinsh/atuin/issues/2200))
- Add npm, Netlify and Pulumi tokens to secret patterns ([#2210](https://github.com/atuinsh/atuin/issues/2210))
- Allow advertising a fake version to clients ([#2228](https://github.com/atuinsh/atuin/issues/2228))
- Monitor idx cache consistency before switching ([#2229](https://github.com/atuinsh/atuin/issues/2229))


### Miscellaneous Tasks

- *(build)* Compile protobufs with protox ([#2122](https://github.com/atuinsh/atuin/issues/2122))
- *(ci)* Do not run current ci for ui ([#2189](https://github.com/atuinsh/atuin/issues/2189))
- *(deps-dev)* Bump @tauri-apps/cli in /ui ([#2135](https://github.com/atuinsh/atuin/issues/2135))
- *(deps-dev)* Bump vite from 5.2.13 to 5.3.1 in /ui ([#2150](https://github.com/atuinsh/atuin/issues/2150))
- *(deps-dev)* Bump @tauri-apps/cli in /ui ([#2277](https://github.com/atuinsh/atuin/issues/2277))
- *(deps-dev)* Bump tailwindcss from 3.4.4 to 3.4.6 in /ui ([#2301](https://github.com/atuinsh/atuin/issues/2301))
- *(install)* Use posix sh, not bash ([#2204](https://github.com/atuinsh/atuin/issues/2204))
- *(nix)* De-couple atuin nix build from nixpkgs rustc version ([#2123](https://github.com/atuinsh/atuin/issues/2123))
- Add installer e2e tests ([#2110](https://github.com/atuinsh/atuin/issues/2110))
- Remove unnecessary proto import ([#2120](https://github.com/atuinsh/atuin/issues/2120))
- Update to rust 1.78
- Add audit config, ignore RUSTSEC-2023-0071 ([#2126](https://github.com/atuinsh/atuin/issues/2126))
- Setup dependabot for the ui ([#2128](https://github.com/atuinsh/atuin/issues/2128))
- Cargo and pnpm update ([#2127](https://github.com/atuinsh/atuin/issues/2127))
- Update to rust 1.79 ([#2138](https://github.com/atuinsh/atuin/issues/2138))
- Update to cargo-dist 0.16, enable attestations ([#2156](https://github.com/atuinsh/atuin/issues/2156))
- Do not use package managers in installer ([#2201](https://github.com/atuinsh/atuin/issues/2201))
- Enable record sync by default ([#2255](https://github.com/atuinsh/atuin/issues/2255))


### Performance

- *(search)* Benchmark smart sort ([#2202](https://github.com/atuinsh/atuin/issues/2202))
- Create idx cache table ([#2140](https://github.com/atuinsh/atuin/issues/2140))
- Write to the idx cache ([#2225](https://github.com/atuinsh/atuin/issues/2225))


### Flake.lock

- Update ([#2213](https://github.com/atuinsh/atuin/issues/2213))


## [18.3.0] - 2024-06-10

### Bug Fixes

- *(bash)* Fix a workaround for bash-5.2 keybindings ([#2060](https://github.com/atuinsh/atuin/issues/2060))
- *(ci)* Release workflow ([#1978](https://github.com/atuinsh/atuin/issues/1978))
- *(client)* Better error reporting on login/registration ([#2076](https://github.com/atuinsh/atuin/issues/2076))
- *(config)* Add quotes for strategy value in comment ([#1993](https://github.com/atuinsh/atuin/issues/1993))
- *(daemon)* Do not try to sync if logged out ([#2037](https://github.com/atuinsh/atuin/issues/2037))
- *(deps)* Replace parse_duration with humantime ([#2074](https://github.com/atuinsh/atuin/issues/2074))
- *(dotfiles)* Alias import with init output ([#1970](https://github.com/atuinsh/atuin/issues/1970))
- *(dotfiles)* Fish alias import ([#1972](https://github.com/atuinsh/atuin/issues/1972))
- *(dotfiles)* More fish alias import ([#1974](https://github.com/atuinsh/atuin/issues/1974))
- *(dotfiles)* Unquote aliases before quoting ([#1976](https://github.com/atuinsh/atuin/issues/1976))
- *(dotfiles)* Allow clearing aliases, disable import ([#1995](https://github.com/atuinsh/atuin/issues/1995))
- *(stats)* Generation for commands starting with a pipe ([#2058](https://github.com/atuinsh/atuin/issues/2058))
- *(ui)* Handle being logged out gracefully ([#2052](https://github.com/atuinsh/atuin/issues/2052))
- *(ui)* Fix mistake in last pr ([#2053](https://github.com/atuinsh/atuin/issues/2053))
- Support not-mac for default shell ([#1960](https://github.com/atuinsh/atuin/issues/1960))
- Adapt help to `enter_accept` config ([#2001](https://github.com/atuinsh/atuin/issues/2001))
- Add protobuf compiler to docker image ([#2009](https://github.com/atuinsh/atuin/issues/2009))
- Add incremental rebuild to daemon loop ([#2010](https://github.com/atuinsh/atuin/issues/2010))
- Alias enable/enabled in settings ([#2021](https://github.com/atuinsh/atuin/issues/2021))
- Bogus error message wording ([#1283](https://github.com/atuinsh/atuin/issues/1283))
- Save sync time in daemon ([#2029](https://github.com/atuinsh/atuin/issues/2029))
- Redact password in database URI when logging ([#2032](https://github.com/atuinsh/atuin/issues/2032))
- Save sync time in daemon ([#2051](https://github.com/atuinsh/atuin/issues/2051))
- Replace serde_yaml::to_string with serde_json::to_string_yaml ([#2087](https://github.com/atuinsh/atuin/issues/2087))


### Documentation

- Fix "From source" `cd` command ([#1973](https://github.com/atuinsh/atuin/issues/1973))
- Add docs for store subcommand ([#2097](https://github.com/atuinsh/atuin/issues/2097))


### Features

- *(daemon)* Add support for daemon on windows ([#2014](https://github.com/atuinsh/atuin/issues/2014))
- *(doctor)* Detect active preexec framework ([#1955](https://github.com/atuinsh/atuin/issues/1955))
- *(doctor)* Report sqlite version ([#2075](https://github.com/atuinsh/atuin/issues/2075))
- *(dotfiles)* Support syncing shell/env vars ([#1977](https://github.com/atuinsh/atuin/issues/1977))
- *(gui)* Work on home page, sort state ([#1956](https://github.com/atuinsh/atuin/issues/1956))
- *(history)* Create atuin-history, add stats to it ([#1990](https://github.com/atuinsh/atuin/issues/1990))
- *(install)* Add Tuxedo OS ([#2018](https://github.com/atuinsh/atuin/issues/2018))
- *(server)* Add me endpoint ([#1954](https://github.com/atuinsh/atuin/issues/1954))
- *(ui)* Scroll history infinitely ([#1999](https://github.com/atuinsh/atuin/issues/1999))
- *(ui)* Add history explore ([#2022](https://github.com/atuinsh/atuin/issues/2022))
- *(ui)* Use correct username on welcome screen ([#2050](https://github.com/atuinsh/atuin/issues/2050))
- *(ui)* Add login/register dialog ([#2056](https://github.com/atuinsh/atuin/issues/2056))
- *(ui)* Setup single-instance ([#2093](https://github.com/atuinsh/atuin/issues/2093))
- *(ui/dotfiles)* Add vars ([#1989](https://github.com/atuinsh/atuin/issues/1989))
- Allow ignoring failed commands ([#1957](https://github.com/atuinsh/atuin/issues/1957))
- Show preview auto ([#1804](https://github.com/atuinsh/atuin/issues/1804))
- Add background daemon ([#2006](https://github.com/atuinsh/atuin/issues/2006))
- Support importing from replxx history files ([#2024](https://github.com/atuinsh/atuin/issues/2024))
- Support systemd socket activation for daemon ([#2039](https://github.com/atuinsh/atuin/issues/2039))


### Miscellaneous Tasks

- *(ci)* Don't run "Update Nix Deps" CI on forks ([#2070](https://github.com/atuinsh/atuin/issues/2070))
- *(codespell)* Ignore CODE_OF_CONDUCT ([#2044](https://github.com/atuinsh/atuin/issues/2044))
- *(install)* Log cargo and rustc version ([#2068](https://github.com/atuinsh/atuin/issues/2068))
- *(release)* V18.3.0-prerelease.1 ([#2090](https://github.com/atuinsh/atuin/issues/2090))
- Move crates into crates/ dir ([#1958](https://github.com/atuinsh/atuin/issues/1958))
- Fix atuin crate readme ([#1959](https://github.com/atuinsh/atuin/issues/1959))
- Add some more logging to handlers ([#1971](https://github.com/atuinsh/atuin/issues/1971))
- Add some more debug logs ([#1979](https://github.com/atuinsh/atuin/issues/1979))
- Clarify default config file ([#2026](https://github.com/atuinsh/atuin/issues/2026))
- Handle rate limited responses ([#2057](https://github.com/atuinsh/atuin/issues/2057))
- Add Systemd config for self-hosted server ([#1879](https://github.com/atuinsh/atuin/issues/1879))
- Switch to cargo dist for releases ([#2085](https://github.com/atuinsh/atuin/issues/2085))
- Update email, gitignore, tweak ui ([#2094](https://github.com/atuinsh/atuin/issues/2094))
- Show scope in changelog ([#2102](https://github.com/atuinsh/atuin/issues/2102))


### Performance

- *(nushell)* Use version.(major|minor|patch) if available ([#1963](https://github.com/atuinsh/atuin/issues/1963))
- Only open the database for commands if strictly required ([#2043](https://github.com/atuinsh/atuin/issues/2043))


### Refactor

- Preview_auto to use enum and different option ([#1991](https://github.com/atuinsh/atuin/issues/1991))


## [18.2.0] - 2024-04-15

### Bug Fixes

- *(bash)* Do not use "return" to cancel initialization ([#1928](https://github.com/atuinsh/atuin/issues/1928))
- *(crate)* Add missing description ([#1861](https://github.com/atuinsh/atuin/issues/1861))
- *(doctor)* Detect preexec plugin using env ATUIN_PREEXEC_BACKEND  ([#1856](https://github.com/atuinsh/atuin/issues/1856))
- *(install)* Install script echo ([#1899](https://github.com/atuinsh/atuin/issues/1899))
- *(nu)* Update atuin.nu to resolve 0.92 deprecation ([#1913](https://github.com/atuinsh/atuin/issues/1913))
- *(search)* Allow empty search ([#1866](https://github.com/atuinsh/atuin/issues/1866))
- *(search)* Case insensitive hostname filtering ([#1883](https://github.com/atuinsh/atuin/issues/1883))
- Pass search query in via env ([#1865](https://github.com/atuinsh/atuin/issues/1865))
- Pass search query in via env for *Nushell* ([#1874](https://github.com/atuinsh/atuin/issues/1874))
- Report non-decodable errors correctly ([#1915](https://github.com/atuinsh/atuin/issues/1915))
- Use spawn_blocking for file access during async context ([#1936](https://github.com/atuinsh/atuin/issues/1936))


### Documentation

- *(bash-preexec)* Describe the limitation of missing commands ([#1937](https://github.com/atuinsh/atuin/issues/1937))
- Add security contact ([#1867](https://github.com/atuinsh/atuin/issues/1867))
- Add install instructions for cave/exherbo linux in README.md ([#1927](https://github.com/atuinsh/atuin/issues/1927))
- Add missing cli help text ([#1945](https://github.com/atuinsh/atuin/issues/1945))


### Features

- *(bash/blesh)* Use _ble_exec_time_ata for duration even in bash < 5 ([#1940](https://github.com/atuinsh/atuin/issues/1940))
- *(dotfiles)* Add alias import ([#1938](https://github.com/atuinsh/atuin/issues/1938))
- *(gui)* Add base structure ([#1935](https://github.com/atuinsh/atuin/issues/1935))
- *(install)* Update install.sh to support KDE Neon ([#1908](https://github.com/atuinsh/atuin/issues/1908))
- *(search)* Process [C-h] and [C-?] as representations of backspace ([#1857](https://github.com/atuinsh/atuin/issues/1857))
- *(search)* Allow specifying search query as an env var ([#1863](https://github.com/atuinsh/atuin/issues/1863))
- *(search)* Add better search scoring ([#1885](https://github.com/atuinsh/atuin/issues/1885))
- *(server)* Check PG version before running migrations ([#1868](https://github.com/atuinsh/atuin/issues/1868))
- Add atuin prefix binding ([#1875](https://github.com/atuinsh/atuin/issues/1875))
- Sync v2 default for new installs ([#1914](https://github.com/atuinsh/atuin/issues/1914))
- Add 'ctrl-a a' to jump to beginning of line ([#1917](https://github.com/atuinsh/atuin/issues/1917))
- Prevents stderr from going to the screen ([#1933](https://github.com/atuinsh/atuin/issues/1933))


### Miscellaneous Tasks

- *(ci)* Add codespell support (config, workflow) and make it fix some typos ([#1916](https://github.com/atuinsh/atuin/issues/1916))
- *(gui)* Cargo update ([#1943](https://github.com/atuinsh/atuin/issues/1943))
- Add issue form ([#1871](https://github.com/atuinsh/atuin/issues/1871))
- Require atuin doctor in issue form ([#1872](https://github.com/atuinsh/atuin/issues/1872))
- Add section to issue form ([#1873](https://github.com/atuinsh/atuin/issues/1873))


### Performance

- *(dotfiles)* Cache aliases and read straight from file ([#1918](https://github.com/atuinsh/atuin/issues/1918))


## [18.1.0] - 2024-03-11

### Bug Fixes

- *(bash)* Rework #1509 to recover from the preexec failure ([#1729](https://github.com/atuinsh/atuin/issues/1729))
- *(build)* Make atuin compile on non-win/mac/linux platforms ([#1825](https://github.com/atuinsh/atuin/issues/1825))
- *(client)* No panic on empty inspector ([#1768](https://github.com/atuinsh/atuin/issues/1768))
- *(doctor)* Use a different method to detect env vars ([#1819](https://github.com/atuinsh/atuin/issues/1819))
- *(dotfiles)* Use latest client ([#1859](https://github.com/atuinsh/atuin/issues/1859))
- *(import/zsh-histdb)* Missing or wrong fields ([#1740](https://github.com/atuinsh/atuin/issues/1740))
- *(nix)* Set meta.mainProgram in the package ([#1823](https://github.com/atuinsh/atuin/issues/1823))
- *(nushell)* Readd up-arrow keybinding, now with menu handling ([#1770](https://github.com/atuinsh/atuin/issues/1770))
- *(regex)* Disable regex error logs ([#1806](https://github.com/atuinsh/atuin/issues/1806))
- *(stats)* Enable multiple command stats to be shown using unicode_segmentation ([#1739](https://github.com/atuinsh/atuin/issues/1739))
- *(store-init)* Re-sync after running auto store init ([#1834](https://github.com/atuinsh/atuin/issues/1834))
- *(sync)* Check store length after sync, not before ([#1805](https://github.com/atuinsh/atuin/issues/1805))
- *(sync)* Record size limiter ([#1827](https://github.com/atuinsh/atuin/issues/1827))
- *(tz)* Attempt to fix timezone reading ([#1810](https://github.com/atuinsh/atuin/issues/1810))
- *(ui)* Don't preserve for empty space ([#1712](https://github.com/atuinsh/atuin/issues/1712))
- *(xonsh)* Add xonsh to auto import, respect $HISTFILE in xonsh import, and fix issue with up-arrow keybinding in xonsh ([#1711](https://github.com/atuinsh/atuin/issues/1711))
- Fish init ([#1725](https://github.com/atuinsh/atuin/issues/1725))
- Typo ([#1741](https://github.com/atuinsh/atuin/issues/1741))
- Check session file exists for status command ([#1756](https://github.com/atuinsh/atuin/issues/1756))
- Ensure sync time is saved for sync v2 ([#1758](https://github.com/atuinsh/atuin/issues/1758))
- Missing characters in preview ([#1803](https://github.com/atuinsh/atuin/issues/1803))
- Doctor shell wording ([#1858](https://github.com/atuinsh/atuin/issues/1858))


### Documentation

- Minor formatting updates to the default config.toml ([#1689](https://github.com/atuinsh/atuin/issues/1689))
- Update docker compose ([#1818](https://github.com/atuinsh/atuin/issues/1818))
- Use db name env variable also in uri ([#1840](https://github.com/atuinsh/atuin/issues/1840))


### Features

- *(client)* Add config option keys.scroll_exits ([#1744](https://github.com/atuinsh/atuin/issues/1744))
- *(dotfiles)* Add enable setting to dotfiles, disable by default ([#1829](https://github.com/atuinsh/atuin/issues/1829))
- *(nix)* Add update action ([#1779](https://github.com/atuinsh/atuin/issues/1779))
- *(nu)* Return early if history is disabled ([#1807](https://github.com/atuinsh/atuin/issues/1807))
- *(nushell)* Add nushell completion generation ([#1791](https://github.com/atuinsh/atuin/issues/1791))
- *(search)* Process Ctrl+m for kitty keyboard protocol ([#1720](https://github.com/atuinsh/atuin/issues/1720))
- *(stats)* Normalize formatting of default config, suggest nix ([#1764](https://github.com/atuinsh/atuin/issues/1764))
- *(stats)* Add linux sysadmin commands to common_subcommands ([#1784](https://github.com/atuinsh/atuin/issues/1784))
- *(ui)* Add config setting for showing tabs ([#1755](https://github.com/atuinsh/atuin/issues/1755))
- Use ATUIN_TEST_SQLITE_STORE_TIMEOUT to specify test timeout of SQLite store ([#1703](https://github.com/atuinsh/atuin/issues/1703))
- Add 'a', 'A', 'h', and 'l' bindings to vim-normal mode ([#1697](https://github.com/atuinsh/atuin/issues/1697))
- Add xonsh history import ([#1678](https://github.com/atuinsh/atuin/issues/1678))
- Add 'ignored_commands' option to stats ([#1722](https://github.com/atuinsh/atuin/issues/1722))
- Support syncing aliases ([#1721](https://github.com/atuinsh/atuin/issues/1721))
- Change fulltext to do multi substring match ([#1660](https://github.com/atuinsh/atuin/issues/1660))
- Add history prune subcommand ([#1743](https://github.com/atuinsh/atuin/issues/1743))
- Add alias feedback and list command ([#1747](https://github.com/atuinsh/atuin/issues/1747))
- Add PHP package manager "composer" to list of default common subcommands ([#1757](https://github.com/atuinsh/atuin/issues/1757))
- Add '/', '?', and 'I' bindings to vim-normal mode ([#1760](https://github.com/atuinsh/atuin/issues/1760))
- Add `CTRL+[` binding as `<Esc>` alias ([#1787](https://github.com/atuinsh/atuin/issues/1787))
- Add atuin doctor ([#1796](https://github.com/atuinsh/atuin/issues/1796))
- Add checks for common setup issues ([#1799](https://github.com/atuinsh/atuin/issues/1799))
- Support regex with r/.../ syntax ([#1745](https://github.com/atuinsh/atuin/issues/1745))
- Guard against ancient versions of bash where this does not work. ([#1794](https://github.com/atuinsh/atuin/issues/1794))
- Add automatic history store init ([#1831](https://github.com/atuinsh/atuin/issues/1831))
- Adds info command to show env vars and config files ([#1841](https://github.com/atuinsh/atuin/issues/1841))


### Miscellaneous Tasks

- *(ci)* Add cross-compile job for illumos ([#1830](https://github.com/atuinsh/atuin/issues/1830))
- *(ci)* Setup nextest ([#1848](https://github.com/atuinsh/atuin/issues/1848))
- Do not show history table stats when using records ([#1835](https://github.com/atuinsh/atuin/issues/1835))


### Performance

- Optimize history init-store ([#1691](https://github.com/atuinsh/atuin/issues/1691))


### Refactor

- *(alias)* Clarify operation result for working with aliases ([#1748](https://github.com/atuinsh/atuin/issues/1748))
- *(nushell)* Update `commandline` syntax, closes #1733 ([#1735](https://github.com/atuinsh/atuin/issues/1735))
- Rename atuin-config to atuin-dotfiles ([#1817](https://github.com/atuinsh/atuin/issues/1817))


## [18.0.1] - 2024-02-12

### Bug Fixes

- Reorder the exit of enhanced keyboard mode ([#1694](https://github.com/atuinsh/atuin/issues/1694))


## [18.0.0] - 2024-02-09

### Bug Fixes

- *(bash)* Avoid unexpected `atuin history start` for keybindings ([#1509](https://github.com/atuinsh/atuin/issues/1509))
- *(bash)* Prevent input to be interpreted as options for blesh auto-complete ([#1511](https://github.com/atuinsh/atuin/issues/1511))
- *(bash)* Work around custom IFS ([#1514](https://github.com/atuinsh/atuin/issues/1514))
- *(bash)* Fix and improve the keybinding to `up` ([#1515](https://github.com/atuinsh/atuin/issues/1515))
- *(bash)* Work around bash < 4 and introduce initialization guards ([#1533](https://github.com/atuinsh/atuin/issues/1533))
- *(bash)* Strip control chars generated by `\[\]` in PS1 with bash-preexec ([#1620](https://github.com/atuinsh/atuin/issues/1620))
- *(bash/preexec)* Erase the prompt last line before Bash renders it
- *(bash/preexec)* Erase the previous prompt before overwriting
- *(bash/preexec)* Support termcap names for tput ([#1670](https://github.com/atuinsh/atuin/issues/1670))
- *(docs)* Update repo url in CONTRIBUTING.md ([#1594](https://github.com/atuinsh/atuin/issues/1594))
- *(fish)* Integration on older fishes ([#1563](https://github.com/atuinsh/atuin/issues/1563))
- *(perm)* Set umask 077 ([#1554](https://github.com/atuinsh/atuin/issues/1554))
- *(search)* Fix invisible tab title ([#1560](https://github.com/atuinsh/atuin/issues/1560))
- *(shell)* Fix incorrect timing of child shells ([#1510](https://github.com/atuinsh/atuin/issues/1510))
- *(sync)* Save sync time when it starts, not ends ([#1573](https://github.com/atuinsh/atuin/issues/1573))
- *(tests)* Add Settings::utc() for utc settings ([#1677](https://github.com/atuinsh/atuin/issues/1677))
- *(tui)* Dedupe was removing history ([#1610](https://github.com/atuinsh/atuin/issues/1610))
- *(windows)* Disables unix specific stuff for windows ([#1557](https://github.com/atuinsh/atuin/issues/1557))
- Prevent input to be interpreted as options for zsh autosuggestions ([#1506](https://github.com/atuinsh/atuin/issues/1506))
- Disable musl deb building ([#1525](https://github.com/atuinsh/atuin/issues/1525))
- Shorten text, use ctrl-o for inspector ([#1561](https://github.com/atuinsh/atuin/issues/1561))
- Print literal control characters to non terminals ([#1586](https://github.com/atuinsh/atuin/issues/1586))
- Escape control characters in command preview ([#1588](https://github.com/atuinsh/atuin/issues/1588))
- Use existing db querying for history list ([#1589](https://github.com/atuinsh/atuin/issues/1589))
- Add acquire timeout to sqlite database connection ([#1590](https://github.com/atuinsh/atuin/issues/1590))
- Only escape control characters when writing to terminal ([#1593](https://github.com/atuinsh/atuin/issues/1593))
- Check for format errors when printing history ([#1623](https://github.com/atuinsh/atuin/issues/1623))
- Skip padding time if it will overflow the allowed prefix length ([#1630](https://github.com/atuinsh/atuin/issues/1630))
- Never overwrite the key ([#1657](https://github.com/atuinsh/atuin/issues/1657))
- Set durability for sqlite to recommended settings ([#1667](https://github.com/atuinsh/atuin/issues/1667))
- Correct download list for incremental builds ([#1672](https://github.com/atuinsh/atuin/issues/1672))


### Documentation

- *(README)* Clarify prerequisites for Bash ([#1686](https://github.com/atuinsh/atuin/issues/1686))
- *(readme)* Add repology badge ([#1494](https://github.com/atuinsh/atuin/issues/1494))
- Add forum link to contributing ([#1498](https://github.com/atuinsh/atuin/issues/1498))
- Refer to image with multi-arch support ([#1513](https://github.com/atuinsh/atuin/issues/1513))
- Remove activity graph
- Fix `Destination file already exists` in Nushell ([#1530](https://github.com/atuinsh/atuin/issues/1530))
- Clarify enter/tab usage ([#1538](https://github.com/atuinsh/atuin/issues/1538))
- Improve style ([#1537](https://github.com/atuinsh/atuin/issues/1537))
- Remove old docusaurus ([#1581](https://github.com/atuinsh/atuin/issues/1581))
- Mention environment variables for custom paths ([#1614](https://github.com/atuinsh/atuin/issues/1614))
- Create pull_request_template.md ([#1632](https://github.com/atuinsh/atuin/issues/1632))
- Update CONTRIBUTING.md ([#1633](https://github.com/atuinsh/atuin/issues/1633))


### Features

- *(bash)* Support high-resolution timing even without ble.sh ([#1534](https://github.com/atuinsh/atuin/issues/1534))
- *(search)* Introduce keymap-dependent vim-mode ([#1570](https://github.com/atuinsh/atuin/issues/1570))
- *(search)* Make cursor style configurable ([#1595](https://github.com/atuinsh/atuin/issues/1595))
- *(shell)* Bind the Atuin search to "/" in vi-normal mode ([#1629](https://github.com/atuinsh/atuin/issues/1629))
  - **BREAKING**: bind the Atuin search to "/" in vi-normal mode ([#1629](https://github.com/atuinsh/atuin/issues/1629))
- *(ui)* Add redraw ([#1519](https://github.com/atuinsh/atuin/issues/1519))
- *(ui)* Vim mode ([#1553](https://github.com/atuinsh/atuin/issues/1553))
- *(ui)* When in vim-normal mode apply an alternative highlighting to the selected line ([#1574](https://github.com/atuinsh/atuin/issues/1574))
- *(zsh)* Update widget names ([#1631](https://github.com/atuinsh/atuin/issues/1631))
- Enable enhanced keyboard mode ([#1505](https://github.com/atuinsh/atuin/issues/1505))
- Rework record sync for improved reliability ([#1478](https://github.com/atuinsh/atuin/issues/1478))
- Include atuin login in secret patterns ([#1518](https://github.com/atuinsh/atuin/issues/1518))
- Make it clear what you are registering for ([#1523](https://github.com/atuinsh/atuin/issues/1523))
- Add extended help ([#1540](https://github.com/atuinsh/atuin/issues/1540))
- Add interactive command inspector ([#1296](https://github.com/atuinsh/atuin/issues/1296))
- Add better error handling for sync ([#1572](https://github.com/atuinsh/atuin/issues/1572))
- Add history rebuild ([#1575](https://github.com/atuinsh/atuin/issues/1575))
- Make deleting from the UI work with record store sync ([#1580](https://github.com/atuinsh/atuin/issues/1580))
- Add metrics counter for records downloaded ([#1584](https://github.com/atuinsh/atuin/issues/1584))
- Make store init idempotent ([#1609](https://github.com/atuinsh/atuin/issues/1609))
- Don't stop with invalid key ([#1612](https://github.com/atuinsh/atuin/issues/1612))
- Add registered and deleted metrics ([#1622](https://github.com/atuinsh/atuin/issues/1622))
- Make history list format configurable ([#1638](https://github.com/atuinsh/atuin/issues/1638))
- Add change-password command & support on server ([#1615](https://github.com/atuinsh/atuin/issues/1615))
- Automatically init history store when record sync is enabled ([#1634](https://github.com/atuinsh/atuin/issues/1634))
- Add store push ([#1649](https://github.com/atuinsh/atuin/issues/1649))
- Reencrypt/rekey local store ([#1662](https://github.com/atuinsh/atuin/issues/1662))
- Add prefers_reduced_motion flag ([#1645](https://github.com/atuinsh/atuin/issues/1645))
- Add verify command to local store
- Add store purge command
- Failure to decrypt history = failure to sync
- Add `store push --force`
- Add `store pull`
- Disable auto record store init ([#1671](https://github.com/atuinsh/atuin/issues/1671))
- Add progress bars to sync and store init ([#1684](https://github.com/atuinsh/atuin/issues/1684))


### Miscellaneous Tasks

- *(ci)* Use github m1 for release builds ([#1658](https://github.com/atuinsh/atuin/issues/1658))
- *(ci)* Re-enable test cache, add separate check step ([#1663](https://github.com/atuinsh/atuin/issues/1663))
- *(ci)* Run rust build/test/check on 3 platforms ([#1675](https://github.com/atuinsh/atuin/issues/1675))
- Remove the teapot response ([#1496](https://github.com/atuinsh/atuin/issues/1496))
- Schema cleanup ([#1522](https://github.com/atuinsh/atuin/issues/1522))
- Update funding ([#1543](https://github.com/atuinsh/atuin/issues/1543))
- Make clipboard dep optional as a feature ([#1558](https://github.com/atuinsh/atuin/issues/1558))
- Add feature to allow always disable check update ([#1628](https://github.com/atuinsh/atuin/issues/1628))
- Use resolver 2, update editions + cargo ([#1635](https://github.com/atuinsh/atuin/issues/1635))
- Disable nix tests ([#1646](https://github.com/atuinsh/atuin/issues/1646))
- Set ATUIN_ variables for development in devshell ([#1653](https://github.com/atuinsh/atuin/issues/1653))


### Refactor

- *(search)* Refactor vim mode ([#1559](https://github.com/atuinsh/atuin/issues/1559))
- *(search)* Refactor handling of key inputs ([#1606](https://github.com/atuinsh/atuin/issues/1606))
- *(shell)* Refactor and localize `HISTORY => __atuin_output` ([#1535](https://github.com/atuinsh/atuin/issues/1535))
- Use enum instead of magic numbers ([#1499](https://github.com/atuinsh/atuin/issues/1499))
- String -> HistoryId ([#1512](https://github.com/atuinsh/atuin/issues/1512))


### Styling

- *(bash)* Use consistent coding style ([#1528](https://github.com/atuinsh/atuin/issues/1528))


### Testing

- Add multi-user integration tests ([#1648](https://github.com/atuinsh/atuin/issues/1648))


### Stats

- Misc improvements ([#1613](https://github.com/atuinsh/atuin/issues/1613))


## [17.2.1] - 2024-01-03

### Bug Fixes

- *(server)* Typo with default config ([#1493](https://github.com/atuinsh/atuin/issues/1493))


## [17.2.0] - 2024-01-03

### Bug Fixes

- *(bash)* Fix loss of the last output line with enter_accept ([#1463](https://github.com/atuinsh/atuin/issues/1463))
- *(bash)* Improve the support for `enter_accept` with `ble.sh` ([#1465](https://github.com/atuinsh/atuin/issues/1465))
- *(bash)* Fix small issues of `enter_accept` for the plain Bash ([#1467](https://github.com/atuinsh/atuin/issues/1467))
- *(bash)* Fix error by the use of ${PS1@P} in bash < 4.4 ([#1488](https://github.com/atuinsh/atuin/issues/1488))
- *(bash,zsh)* Fix quirks on search cancel ([#1483](https://github.com/atuinsh/atuin/issues/1483))
- *(clippy)* Ignore struct_field_names ([#1466](https://github.com/atuinsh/atuin/issues/1466))
- *(docs)* Fix typo ([#1439](https://github.com/atuinsh/atuin/issues/1439))
- *(docs)* Discord link expired
- *(history)* Disallow deletion if the '--limit' flag is present ([#1436](https://github.com/atuinsh/atuin/issues/1436))
- *(import/zsh)* Zsh use a special format to escape some characters ([#1490](https://github.com/atuinsh/atuin/issues/1490))
- *(install)* Discord broken link
- *(shell)* Respect ZSH's $ZDOTDIR environment variable ([#1441](https://github.com/atuinsh/atuin/issues/1441))
- *(stats)* Don't require all fields under [stats] ([#1437](https://github.com/atuinsh/atuin/issues/1437))
- *(stats)* Time now_local not working 
- *(zsh)* Zsh_autosuggest_strategy for no-unset environment ([#1486](https://github.com/atuinsh/atuin/issues/1486))


### Documentation

- *(readme)* Add actuated linkback
- *(readme)* Fix light/dark mode logo
- *(readme)* Use picture element for logo
- Add link to forum
- Align setup links in docs and readme ([#1446](https://github.com/atuinsh/atuin/issues/1446))
- Add Void Linux install instruction ([#1445](https://github.com/atuinsh/atuin/issues/1445))
- Add fish install script ([#1447](https://github.com/atuinsh/atuin/issues/1447))
- Correct link
- Add docs for zsh-autosuggestion integration ([#1480](https://github.com/atuinsh/atuin/issues/1480))
- Remove stray character from README
- Update logo ([#1481](https://github.com/atuinsh/atuin/issues/1481))


### Features

- *(bash)* Provide auto-complete source for ble.sh ([#1487](https://github.com/atuinsh/atuin/issues/1487))
- *(shell)* Support high-resolution duration if available ([#1484](https://github.com/atuinsh/atuin/issues/1484))
- Add semver checking to client requests ([#1456](https://github.com/atuinsh/atuin/issues/1456))
- Add TLS to atuin-server ([#1457](https://github.com/atuinsh/atuin/issues/1457))
- Integrate with zsh-autosuggestions ([#1479](https://github.com/atuinsh/atuin/issues/1479))


### Miscellaneous Tasks

- *(repo)* Remove issue config ([#1433](https://github.com/atuinsh/atuin/issues/1433))
- Remove issue template ([#1444](https://github.com/atuinsh/atuin/issues/1444))


### Refactor

- *(bash)* Factorize `__atuin_accept_line` ([#1476](https://github.com/atuinsh/atuin/issues/1476))
- *(bash)* Refactor and optimize `__atuin_accept_line` ([#1482](https://github.com/atuinsh/atuin/issues/1482))


## [17.1.0] - 2023-12-10

### Bug Fixes

- *(fish)* Clean up the fish script options ([#1370](https://github.com/atuinsh/atuin/issues/1370))
- *(fish)* Use fish builtins for `enter_accept` ([#1373](https://github.com/atuinsh/atuin/issues/1373))
- *(fish)* Accept multiline commands ([#1418](https://github.com/atuinsh/atuin/issues/1418))
- *(nix)* Add Appkit to the package build ([#1358](https://github.com/atuinsh/atuin/issues/1358))
- *(zsh)* Bind in the most popular modes ([#1360](https://github.com/atuinsh/atuin/issues/1360))
- *(zsh)* Only trigger up-arrow on first line ([#1359](https://github.com/atuinsh/atuin/issues/1359))
- Initial list of history in workspace mode ([#1356](https://github.com/atuinsh/atuin/issues/1356))
- Make `atuin account delete` void session + key ([#1393](https://github.com/atuinsh/atuin/issues/1393))
- New clippy lints ([#1395](https://github.com/atuinsh/atuin/issues/1395))
- Reenable enter_accept for bash ([#1408](https://github.com/atuinsh/atuin/issues/1408))
- Respect ZSH's $ZDOTDIR environment variable ([#942](https://github.com/atuinsh/atuin/issues/942))


### Documentation

- Update sync.md ([#1409](https://github.com/atuinsh/atuin/issues/1409))
- Update Arch Linux package URL in advanced-install.md ([#1407](https://github.com/atuinsh/atuin/issues/1407))
- New stats config ([#1412](https://github.com/atuinsh/atuin/issues/1412))


### Features

- *(nix)* Add a nixpkgs overlay ([#1357](https://github.com/atuinsh/atuin/issues/1357))
- Add metrics server and http metrics ([#1394](https://github.com/atuinsh/atuin/issues/1394))
- Add some metrics related to Atuin as an app ([#1399](https://github.com/atuinsh/atuin/issues/1399))
- Allow configuring stats prefix ([#1411](https://github.com/atuinsh/atuin/issues/1411))
- Allow spaces in stats prefixes ([#1414](https://github.com/atuinsh/atuin/issues/1414))


### Miscellaneous Tasks

- *(readme)* Add contributor image to README ([#1430](https://github.com/atuinsh/atuin/issues/1430))
- Update to sqlx 0.7.3 ([#1416](https://github.com/atuinsh/atuin/issues/1416))
- `cargo update` ([#1419](https://github.com/atuinsh/atuin/issues/1419))
- Update rusty_paseto and rusty_paserk ([#1420](https://github.com/atuinsh/atuin/issues/1420))
- Run dependabot weekly, not daily ([#1423](https://github.com/atuinsh/atuin/issues/1423))
- Don't group deps ([#1424](https://github.com/atuinsh/atuin/issues/1424))
- Setup git cliff ([#1431](https://github.com/atuinsh/atuin/issues/1431))


## [17.0.1] - 2023-10-28

### Bug Fixes

- *(bash)* Improve output of `enter_accept` ([#1342](https://github.com/atuinsh/atuin/issues/1342))
- *(enter_accept)* Clear old cmd snippet ([#1350](https://github.com/atuinsh/atuin/issues/1350))
- *(fish)* Improve output for `enter_accept` ([#1341](https://github.com/atuinsh/atuin/issues/1341))


## [17.0.0] - 2023-10-26

### Bug Fixes

- *(1220)* Workspace Filtermode not handled in skim engine ([#1273](https://github.com/atuinsh/atuin/issues/1273))
- *(nu)* Disable the up-arrow keybinding for Nushell ([#1329](https://github.com/atuinsh/atuin/issues/1329))
- *(nushell)* Ignore stderr messages ([#1320](https://github.com/atuinsh/atuin/issues/1320))
- *(ubuntu/arm*)* Detect non amd64 ubuntu and handle ([#1131](https://github.com/atuinsh/atuin/issues/1131))


### Documentation

- Update `workspace` config key to `workspaces` ([#1174](https://github.com/atuinsh/atuin/issues/1174))
- Document the available format options of History list command ([#1234](https://github.com/atuinsh/atuin/issues/1234))


### Features

- *(installer)* Try installing via paru for the AUR ([#1262](https://github.com/atuinsh/atuin/issues/1262))
- *(keyup)* Configure SearchMode for KeyUp invocation #1216 ([#1224](https://github.com/atuinsh/atuin/issues/1224))
- Mouse selection support ([#1209](https://github.com/atuinsh/atuin/issues/1209))
- Copy to clipboard ([#1249](https://github.com/atuinsh/atuin/issues/1249))


### Refactor

- Duplications reduced in order to align implementations of reading history files ([#1247](https://github.com/atuinsh/atuin/issues/1247))


### Config.md

- Invert mode detailed options ([#1225](https://github.com/atuinsh/atuin/issues/1225))


## [16.0.0] - 2023-08-07

### Bug Fixes

- *(docs)* List all presently documented commands ([#1140](https://github.com/atuinsh/atuin/issues/1140))
- *(docs)* Correct command overview paths ([#1145](https://github.com/atuinsh/atuin/issues/1145))
- *(server)* Teapot is a cup of coffee ([#1137](https://github.com/atuinsh/atuin/issues/1137))
- Adjust broken link to supported shells ([#1013](https://github.com/atuinsh/atuin/issues/1013))
- Fixes unix specific impl of shutdown_signal ([#1061](https://github.com/atuinsh/atuin/issues/1061))
- Nushell empty hooks ([#1138](https://github.com/atuinsh/atuin/issues/1138))


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

- *(client)* Always read session_path from settings ([#757](https://github.com/atuinsh/atuin/issues/757))
- *(installer)* Use case-insensitive comparison ([#776](https://github.com/atuinsh/atuin/issues/776))
- Many wins were broken :memo: ([#789](https://github.com/atuinsh/atuin/issues/789))
- Paste into terminal after switching modes ([#793](https://github.com/atuinsh/atuin/issues/793))
- Record negative exit codes ([#821](https://github.com/atuinsh/atuin/issues/821))
- Allow nix package to fetch dependencies from git ([#832](https://github.com/atuinsh/atuin/issues/832))


### Documentation

- *(README)* Fix activity graph link ([#753](https://github.com/atuinsh/atuin/issues/753))


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

- *(README)* Add static activity graph example ([#680](https://github.com/atuinsh/atuin/issues/680))
- Remove human short flag from docs, duplicate of help -h ([#663](https://github.com/atuinsh/atuin/issues/663))
- Fix typo in zh-CN/README.md ([#666](https://github.com/atuinsh/atuin/issues/666))


### Features

- *(history)* Add new flag to allow custom output format ([#662](https://github.com/atuinsh/atuin/issues/662))


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
