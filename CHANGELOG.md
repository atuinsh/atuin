# Changelog

All notable changes to this project will be documented in this file.

## [18.1.0] - 2024-03-11

### Bug Fixes

- Don't preserve for empty space ([#1712](https://github.com/atuinsh/atuin/issues/1712))
- Fish init ([#1725](https://github.com/atuinsh/atuin/issues/1725))
- Add xonsh to auto import, respect $HISTFILE in xonsh import, and fix issue with up-arrow keybinding in xonsh ([#1711](https://github.com/atuinsh/atuin/issues/1711))
- Rework #1509 to recover from the preexec failure ([#1729](https://github.com/atuinsh/atuin/issues/1729))
- Typo ([#1741](https://github.com/atuinsh/atuin/issues/1741))
- Missing or wrong fields ([#1740](https://github.com/atuinsh/atuin/issues/1740))
- Check session file exists for status command ([#1756](https://github.com/atuinsh/atuin/issues/1756))
- Ensure sync time is saved for sync v2 ([#1758](https://github.com/atuinsh/atuin/issues/1758))
- No panic on empty inspector ([#1768](https://github.com/atuinsh/atuin/issues/1768))
- Enable multiple command stats to be shown using unicode_segmentation ([#1739](https://github.com/atuinsh/atuin/issues/1739))
- Readd up-arrow keybinding, now with menu handling ([#1770](https://github.com/atuinsh/atuin/issues/1770))
- Missing characters in preview ([#1803](https://github.com/atuinsh/atuin/issues/1803))
- Check store length after sync, not before ([#1805](https://github.com/atuinsh/atuin/issues/1805))
- Disable regex error logs ([#1806](https://github.com/atuinsh/atuin/issues/1806))
- Attempt to fix timezone reading ([#1810](https://github.com/atuinsh/atuin/issues/1810))
- Use a different method to detect env vars ([#1819](https://github.com/atuinsh/atuin/issues/1819))
- Record size limiter ([#1827](https://github.com/atuinsh/atuin/issues/1827))
- Make atuin compile on non-win/mac/linux platforms ([#1825](https://github.com/atuinsh/atuin/issues/1825))
- Set meta.mainProgram in the package ([#1823](https://github.com/atuinsh/atuin/issues/1823))
- Re-sync after running auto store init ([#1834](https://github.com/atuinsh/atuin/issues/1834))

### Documentation

- Minor formatting updates to the default config.toml ([#1689](https://github.com/atuinsh/atuin/issues/1689))
- Update docker compose ([#1818](https://github.com/atuinsh/atuin/issues/1818))
- Use db name env variable also in uri ([#1840](https://github.com/atuinsh/atuin/issues/1840))

### Features

- Use ATUIN_TEST_SQLITE_STORE_TIMEOUT to specify test timeout of SQLite store ([#1703](https://github.com/atuinsh/atuin/issues/1703))
- Add 'a', 'A', 'h', and 'l' bindings to vim-normal mode ([#1697](https://github.com/atuinsh/atuin/issues/1697))
- Add xonsh history import ([#1678](https://github.com/atuinsh/atuin/issues/1678))
- Process Ctrl+m for kitty keyboard protocol ([#1720](https://github.com/atuinsh/atuin/issues/1720))
- Add 'ignored_commands' option to stats ([#1722](https://github.com/atuinsh/atuin/issues/1722))
- Support syncing aliases ([#1721](https://github.com/atuinsh/atuin/issues/1721))
- Change fulltext to do multi substring match ([#1660](https://github.com/atuinsh/atuin/issues/1660))
- Add config option keys.scroll_exits ([#1744](https://github.com/atuinsh/atuin/issues/1744))
- Add history prune subcommand ([#1743](https://github.com/atuinsh/atuin/issues/1743))
- Add alias feedback and list command ([#1747](https://github.com/atuinsh/atuin/issues/1747))
- Add PHP package manager "composer" to list of default common subcommands ([#1757](https://github.com/atuinsh/atuin/issues/1757))
- Add '/', '?', and 'I' bindings to vim-normal mode ([#1760](https://github.com/atuinsh/atuin/issues/1760))
- Add update action ([#1779](https://github.com/atuinsh/atuin/issues/1779))
- Normalize formatting of default config, suggest nix ([#1764](https://github.com/atuinsh/atuin/issues/1764))
- Add linux sysadmin commands to common_subcommands ([#1784](https://github.com/atuinsh/atuin/issues/1784))
- Add `CTRL+[` binding as `<Esc>` alias ([#1787](https://github.com/atuinsh/atuin/issues/1787))
- Add nushell completion generation ([#1791](https://github.com/atuinsh/atuin/issues/1791))
- Add atuin doctor ([#1796](https://github.com/atuinsh/atuin/issues/1796))
- Add checks for common setup issues ([#1799](https://github.com/atuinsh/atuin/issues/1799))
- Support regex with r/.../ syntax ([#1745](https://github.com/atuinsh/atuin/issues/1745))
- Guard against ancient versions of bash where this does not work. ([#1794](https://github.com/atuinsh/atuin/issues/1794))
- Add config setting for showing tabs ([#1755](https://github.com/atuinsh/atuin/issues/1755))
- Return early if history is disabled ([#1807](https://github.com/atuinsh/atuin/issues/1807))
- Add enable setting to dotfiles, disable by default ([#1829](https://github.com/atuinsh/atuin/issues/1829))
- Add automatic history store init ([#1831](https://github.com/atuinsh/atuin/issues/1831))
- Adds info command to show env vars and config files ([#1841](https://github.com/atuinsh/atuin/issues/1841))

### Miscellaneous Tasks

- Add cross-compile job for illumos ([#1830](https://github.com/atuinsh/atuin/issues/1830))
- Do not show history table stats when using records ([#1835](https://github.com/atuinsh/atuin/issues/1835))
- Setup nextest ([#1848](https://github.com/atuinsh/atuin/issues/1848))

### Performance

- Optimize history init-store ([#1691](https://github.com/atuinsh/atuin/issues/1691))

### Refactor

- Update `commandline` syntax, closes #1733 ([#1735](https://github.com/atuinsh/atuin/issues/1735))
- Clarify operation result for working with aliases ([#1748](https://github.com/atuinsh/atuin/issues/1748))
- Rename atuin-config to atuin-dotfiles ([#1817](https://github.com/atuinsh/atuin/issues/1817))

## [18.0.1] - 2024-02-12

### Bug Fixes

- Reorder the exit of enhanced keyboard mode ([#1694](https://github.com/atuinsh/atuin/issues/1694))

## [18.0.0] - 2024-02-09

### Bug Fixes

- Prevent input to be interpreted as options for zsh autosuggestions ([#1506](https://github.com/atuinsh/atuin/issues/1506))
- Avoid unexpected `atuin history start` for keybindings ([#1509](https://github.com/atuinsh/atuin/issues/1509))
- Prevent input to be interpreted as options for blesh auto-complete ([#1511](https://github.com/atuinsh/atuin/issues/1511))
- Work around custom IFS ([#1514](https://github.com/atuinsh/atuin/issues/1514))
- Fix and improve the keybinding to `up` ([#1515](https://github.com/atuinsh/atuin/issues/1515))
- Fix incorrect timing of child shells ([#1510](https://github.com/atuinsh/atuin/issues/1510))
- Disable musl deb building ([#1525](https://github.com/atuinsh/atuin/issues/1525))
- Work around bash < 4 and introduce initialization guards ([#1533](https://github.com/atuinsh/atuin/issues/1533))
- Set umask 077 ([#1554](https://github.com/atuinsh/atuin/issues/1554))
- Disables unix specific stuff for windows ([#1557](https://github.com/atuinsh/atuin/issues/1557))
- Fix invisible tab title ([#1560](https://github.com/atuinsh/atuin/issues/1560))
- Shorten text, use ctrl-o for inspector ([#1561](https://github.com/atuinsh/atuin/issues/1561))
- Integration on older fishes ([#1563](https://github.com/atuinsh/atuin/issues/1563))
- Save sync time when it starts, not ends ([#1573](https://github.com/atuinsh/atuin/issues/1573))
- Print literal control characters to non terminals ([#1586](https://github.com/atuinsh/atuin/issues/1586))
- Escape control characters in command preview ([#1588](https://github.com/atuinsh/atuin/issues/1588))
- Use existing db querying for history list ([#1589](https://github.com/atuinsh/atuin/issues/1589))
- Add acquire timeout to sqlite database connection ([#1590](https://github.com/atuinsh/atuin/issues/1590))
- Update repo url in CONTRIBUTING.md ([#1594](https://github.com/atuinsh/atuin/issues/1594))
- Dedupe was removing history ([#1610](https://github.com/atuinsh/atuin/issues/1610))
- Only escape control characters when writing to terminal ([#1593](https://github.com/atuinsh/atuin/issues/1593))
- Strip control chars generated by `\[\]` in PS1 with bash-preexec ([#1620](https://github.com/atuinsh/atuin/issues/1620))
- Check for format errors when printing history ([#1623](https://github.com/atuinsh/atuin/issues/1623))
- Skip padding time if it will overflow the allowed prefix length ([#1630](https://github.com/atuinsh/atuin/issues/1630))
- Never overwrite the key ([#1657](https://github.com/atuinsh/atuin/issues/1657))
- Erase the prompt last line before Bash renders it
- Erase the previous prompt before overwriting
- Support termcap names for tput ([#1670](https://github.com/atuinsh/atuin/issues/1670))
- Set durability for sqlite to recommended settings ([#1667](https://github.com/atuinsh/atuin/issues/1667))
- Correct download list for incremental builds ([#1672](https://github.com/atuinsh/atuin/issues/1672))
- Add Settings::utc() for utc settings ([#1677](https://github.com/atuinsh/atuin/issues/1677))

### Documentation

- Add repology badge ([#1494](https://github.com/atuinsh/atuin/issues/1494))
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
- Clarify prerequisites for Bash ([#1686](https://github.com/atuinsh/atuin/issues/1686))

### Features

- Enable enhanced keyboard mode ([#1505](https://github.com/atuinsh/atuin/issues/1505))
- Rework record sync for improved reliability ([#1478](https://github.com/atuinsh/atuin/issues/1478))
- Include atuin login in secret patterns ([#1518](https://github.com/atuinsh/atuin/issues/1518))
- Add redraw ([#1519](https://github.com/atuinsh/atuin/issues/1519))
- Make it clear what you are registering for ([#1523](https://github.com/atuinsh/atuin/issues/1523))
- Support high-resolution timing even without ble.sh ([#1534](https://github.com/atuinsh/atuin/issues/1534))
- Add extended help ([#1540](https://github.com/atuinsh/atuin/issues/1540))
- Add interactive command inspector ([#1296](https://github.com/atuinsh/atuin/issues/1296))
- Vim mode ([#1553](https://github.com/atuinsh/atuin/issues/1553))
- Add better error handling for sync ([#1572](https://github.com/atuinsh/atuin/issues/1572))
- Add history rebuild ([#1575](https://github.com/atuinsh/atuin/issues/1575))
- Introduce keymap-dependent vim-mode ([#1570](https://github.com/atuinsh/atuin/issues/1570))
- Make deleting from the UI work with record store sync ([#1580](https://github.com/atuinsh/atuin/issues/1580))
- Add metrics counter for records downloaded ([#1584](https://github.com/atuinsh/atuin/issues/1584))
- Make cursor style configurable ([#1595](https://github.com/atuinsh/atuin/issues/1595))
- Make store init idempotent ([#1609](https://github.com/atuinsh/atuin/issues/1609))
- Don't stop with invalid key ([#1612](https://github.com/atuinsh/atuin/issues/1612))
- Add registered and deleted metrics ([#1622](https://github.com/atuinsh/atuin/issues/1622))
- When in vim-normal mode apply an alternative highlighting to the selected line ([#1574](https://github.com/atuinsh/atuin/issues/1574))
- [**breaking**] Bind the Atuin search to "/" in vi-normal mode ([#1629](https://github.com/atuinsh/atuin/issues/1629))
- Update widget names ([#1631](https://github.com/atuinsh/atuin/issues/1631))
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

- Remove the teapot response ([#1496](https://github.com/atuinsh/atuin/issues/1496))
- Schema cleanup ([#1522](https://github.com/atuinsh/atuin/issues/1522))
- Update funding ([#1543](https://github.com/atuinsh/atuin/issues/1543))
- Make clipboard dep optional as a feature ([#1558](https://github.com/atuinsh/atuin/issues/1558))
- Add feature to allow always disable check update ([#1628](https://github.com/atuinsh/atuin/issues/1628))
- Use resolver 2, update editions + cargo ([#1635](https://github.com/atuinsh/atuin/issues/1635))
- Disable nix tests ([#1646](https://github.com/atuinsh/atuin/issues/1646))
- Set ATUIN_ variables for development in devshell ([#1653](https://github.com/atuinsh/atuin/issues/1653))
- Use github m1 for release builds ([#1658](https://github.com/atuinsh/atuin/issues/1658))
- Re-enable test cache, add separate check step ([#1663](https://github.com/atuinsh/atuin/issues/1663))
- Run rust build/test/check on 3 platforms ([#1675](https://github.com/atuinsh/atuin/issues/1675))

### Refactor

- Use enum instead of magic numbers ([#1499](https://github.com/atuinsh/atuin/issues/1499))
- String -> HistoryId ([#1512](https://github.com/atuinsh/atuin/issues/1512))
- Refactor and localize `HISTORY => __atuin_output` ([#1535](https://github.com/atuinsh/atuin/issues/1535))
- Refactor vim mode ([#1559](https://github.com/atuinsh/atuin/issues/1559))
- Refactor handling of key inputs ([#1606](https://github.com/atuinsh/atuin/issues/1606))

### Styling

- Use consistent coding style ([#1528](https://github.com/atuinsh/atuin/issues/1528))

### Testing

- Add multi-user integration tests ([#1648](https://github.com/atuinsh/atuin/issues/1648))

### Stats

- Misc improvements ([#1613](https://github.com/atuinsh/atuin/issues/1613))

## [17.2.1] - 2024-01-03

### Bug Fixes

- Typo with default config ([#1493](https://github.com/atuinsh/atuin/issues/1493))

## [17.2.0] - 2024-01-03

### Bug Fixes

- Fix typo ([#1439](https://github.com/atuinsh/atuin/issues/1439))
- Don't require all fields under [stats] ([#1437](https://github.com/atuinsh/atuin/issues/1437))
- Disallow deletion if the '--limit' flag is present ([#1436](https://github.com/atuinsh/atuin/issues/1436))
- Respect ZSH's $ZDOTDIR environment variable ([#1441](https://github.com/atuinsh/atuin/issues/1441))
- Fix loss of the last output line with enter_accept ([#1463](https://github.com/atuinsh/atuin/issues/1463))
- Ignore struct_field_names ([#1466](https://github.com/atuinsh/atuin/issues/1466))
- Improve the support for `enter_accept` with `ble.sh` ([#1465](https://github.com/atuinsh/atuin/issues/1465))
- Discord link expired
- Discord broken link
- Fix small issues of `enter_accept` for the plain Bash ([#1467](https://github.com/atuinsh/atuin/issues/1467))
- Time now_local not working 
- Fix quirks on search cancel ([#1483](https://github.com/atuinsh/atuin/issues/1483))
- Zsh_autosuggest_strategy for no-unset environment ([#1486](https://github.com/atuinsh/atuin/issues/1486))
- Fix error by the use of ${PS1@P} in bash < 4.4 ([#1488](https://github.com/atuinsh/atuin/issues/1488))
- Zsh use a special format to escape some characters ([#1490](https://github.com/atuinsh/atuin/issues/1490))

### Documentation

- Add actuated linkback
- Add link to forum
- Align setup links in docs and readme ([#1446](https://github.com/atuinsh/atuin/issues/1446))
- Add Void Linux install instruction ([#1445](https://github.com/atuinsh/atuin/issues/1445))
- Add fish install script ([#1447](https://github.com/atuinsh/atuin/issues/1447))
- Correct link
- Fix light/dark mode logo
- Use picture element for logo
- Add docs for zsh-autosuggestion integration ([#1480](https://github.com/atuinsh/atuin/issues/1480))
- Remove stray character from README
- Update logo ([#1481](https://github.com/atuinsh/atuin/issues/1481))

### Features

- Add semver checking to client requests ([#1456](https://github.com/atuinsh/atuin/issues/1456))
- Add TLS to atuin-server ([#1457](https://github.com/atuinsh/atuin/issues/1457))
- Integrate with zsh-autosuggestions ([#1479](https://github.com/atuinsh/atuin/issues/1479))
- Support high-resolution duration if available ([#1484](https://github.com/atuinsh/atuin/issues/1484))
- Provide auto-complete source for ble.sh ([#1487](https://github.com/atuinsh/atuin/issues/1487))

### Miscellaneous Tasks

- Remove issue config ([#1433](https://github.com/atuinsh/atuin/issues/1433))
- Remove issue template ([#1444](https://github.com/atuinsh/atuin/issues/1444))

### Refactor

- Factorize `__atuin_accept_line` ([#1476](https://github.com/atuinsh/atuin/issues/1476))
- Refactor and optimize `__atuin_accept_line` ([#1482](https://github.com/atuinsh/atuin/issues/1482))

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
