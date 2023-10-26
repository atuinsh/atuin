---
title: Atuin v17
description: Release notes for Atuin v17! 
slug: release-v17
authors: [ellie]
tags: [release]
---

Announcing a new release of Atuin! v17 is out now.

### Community

- [Discord](https://discord.gg/Fq8bJSKPHh)
- [Mastodon](https://hachyderm.io/@atuin)
- [Twitter](https://twitter.com/atuinsh)

## Self hosted changes

We are no longer building docker images for `main`, and all images are now tagged either by release (`17.0.0`) or by short commit sha (`1a20afe`). 

We advise that users stick to running tagged releases, and do not track an unstable branch. If you wish to run potentially unstable and unreleased code, then please do watch the repo and keep your install up to date!

We now also build docker images for ARM! This has been an issue for a long time for us, as GitHub does not provide ARM runners + emulation is very very slow. Thank you so much to @alexellis and @self-actuated for helping us out there!

## `enter_accept` and keybinding changes

For a long time, we have been asked about Atuin requiring two enter presses - once to select the search item, and then once more to run it from your shell. While some users were happy with this, many felt that the additional keypress slowed them down unnecessarily.

v17 introduces the `enter_accept` config option. If set to `true`, pressing enter will immediately select and execute the search result selected. If you'd rather select the item and then edit it in your shell, you can press tab instead.

This is enabled by default for _new_ users only. Existing users will need to edit their config. Currently, this does not support NuShell.

We have also temporarily disabled the "up" arrow keybinding by default for NuShell, while awaiting an upstream fix, see #1329 for more.

## What's Changed
* Fix client-only builds by @ellie in https://github.com/atuinsh/atuin/pull/1155
* Update(docs) Add `workspace` to config.toml and config.md by @thePanz in https://github.com/atuinsh/atuin/pull/1157
* Bump lukemathwalker/cargo-chef from latest-rust-1.71.0 to latest-rust-1.71.1 by @dependabot in https://github.com/atuinsh/atuin/pull/1154
* Fix index tail leak by @ellie in https://github.com/atuinsh/atuin/pull/1159
* Include revision in status by @ellie in https://github.com/atuinsh/atuin/pull/1166
* Run check for client-only feature set by @tobiasge in https://github.com/atuinsh/atuin/pull/1167
* Fix nix build by @ellie in https://github.com/atuinsh/atuin/pull/1171
* Update to ratatui 0.22 by @ellie in https://github.com/atuinsh/atuin/pull/1168
* Remove terminal mode switching by @ellie in https://github.com/atuinsh/atuin/pull/1170
* Only setup shell plugin if it's not already there by @ellie in https://github.com/atuinsh/atuin/pull/1178
* docs: update `workspace` config key to `workspaces` by @tombh in https://github.com/atuinsh/atuin/pull/1174
* Bump debian from bullseye-20230703-slim to bullseye-20230814-slim by @dependabot in https://github.com/atuinsh/atuin/pull/1176
* Fix keybinding link in README by @edwardloveall in https://github.com/atuinsh/atuin/pull/1173
* fix(ubuntu/arm*): detect non amd64 ubuntu and handle by @jinnko in https://github.com/atuinsh/atuin/pull/1131
* Add kv map builder and list function by @ellie in https://github.com/atuinsh/atuin/pull/1179
* Dependency updates by @conradludgate in https://github.com/atuinsh/atuin/pull/1181
* Automatically filter out secrets by @ellie in https://github.com/atuinsh/atuin/pull/1182
* Remove fig from README by @ellie in https://github.com/atuinsh/atuin/pull/1197
* Run formatting by @ellie in https://github.com/atuinsh/atuin/pull/1202
* Bump lukemathwalker/cargo-chef from latest-rust-1.71.1 to latest-rust-1.72.0 by @dependabot in https://github.com/atuinsh/atuin/pull/1196
* Explicitly use buster image for cargo-chef, mitigates #1204 by @Artanicus in https://github.com/atuinsh/atuin/pull/1205
* feat: mouse selection support by @YummyOreo in https://github.com/atuinsh/atuin/pull/1209
* Use `case` for Linux distro choice in `install.sh` by @mentalisttraceur in https://github.com/atuinsh/atuin/pull/1200
* replace chrono with time by @conradludgate in https://github.com/atuinsh/atuin/pull/806
* Run `cargo update` by @ellie in https://github.com/atuinsh/atuin/pull/1218
* Move contributors list to top-level file by @utterstep in https://github.com/atuinsh/atuin/pull/931
* Bump itertools from 0.10.5 to 0.11.0 by @dependabot in https://github.com/atuinsh/atuin/pull/1223
* Bump crossterm from 0.26.1 to 0.27.0 by @dependabot in https://github.com/atuinsh/atuin/pull/1222
* Bump debian from bullseye-20230814-slim to bullseye-20230904-slim by @dependabot in https://github.com/atuinsh/atuin/pull/1213
* Bump tower-http from 0.3.5 to 0.4.4 by @dependabot in https://github.com/atuinsh/atuin/pull/1210
* Bump shellexpand from 2.1.2 to 3.1.0 by @dependabot in https://github.com/atuinsh/atuin/pull/1186
* Bump ratatui from 0.22.0 to 0.23.0 by @dependabot in https://github.com/atuinsh/atuin/pull/1221
* Update config.toml: List inverted mode by @mateuscomh in https://github.com/atuinsh/atuin/pull/1226
* config.md: invert mode detailed options by @mateuscomh in https://github.com/atuinsh/atuin/pull/1225
* docs: document the available format options of History list command by @deicon in https://github.com/atuinsh/atuin/pull/1234
* Fix selecting complex fish commands by @ellie in https://github.com/atuinsh/atuin/pull/1237
* feat(keyup): Configure SearchMode for KeyUp invocation #1216 by @deicon in https://github.com/atuinsh/atuin/pull/1224
* Add connect timeout and overall timeout by @ellie in https://github.com/atuinsh/atuin/pull/1238
* Bump debian from bullseye-20230904-slim to bullseye-20230919-slim by @dependabot in https://github.com/atuinsh/atuin/pull/1242
* Refactor/duplicates removed by @deicon in https://github.com/atuinsh/atuin/pull/1247
* better sync error messages by @conradludgate in https://github.com/atuinsh/atuin/pull/1254
* handle missing entries (fixes #1236) by @conradludgate in https://github.com/atuinsh/atuin/pull/1253
* feat(installer): try installing via paru for the AUR by @orhun in https://github.com/atuinsh/atuin/pull/1262
* Add support template by @ellie in https://github.com/atuinsh/atuin/pull/1267
* Update support.yml by @ellie in https://github.com/atuinsh/atuin/pull/1268
* fix sync timestamps by @conradludgate in https://github.com/atuinsh/atuin/pull/1258
* add --reverse to history list by @kiran-4444 in https://github.com/atuinsh/atuin/pull/1252
* handle empty keybindings list for nushell by @dcarosone in https://github.com/atuinsh/atuin/pull/1270
* calendar timezones by @conradludgate in https://github.com/atuinsh/atuin/pull/1259
* feat: copy to clipboard by @YummyOreo in https://github.com/atuinsh/atuin/pull/1249
* Re-enable `linux/arm64` platform in CI docker build by @rriski in https://github.com/atuinsh/atuin/pull/1276
* Revert "Re-enable `linux/arm64` platform in CI docker build" by @ellie in https://github.com/atuinsh/atuin/pull/1278
* Use github runners for unit tests (for now) by @ellie in https://github.com/atuinsh/atuin/pull/1279
* Add --print0 to `history list` by @offbyone in https://github.com/atuinsh/atuin/pull/1274
* A man is not dead while his name is still spoken by @offbyone in https://github.com/atuinsh/atuin/pull/1280
* Fix/1207 deleted entries shown in interactive search by @deicon in https://github.com/atuinsh/atuin/pull/1272
* fix(1220): Workspace Filtermode not handled in skim engine by @deicon in https://github.com/atuinsh/atuin/pull/1273
* clear history id by @conradludgate in https://github.com/atuinsh/atuin/pull/1263
* Revert "Use github runners for unit tests (for now)" by @ellie in https://github.com/atuinsh/atuin/pull/1294
* Revert "Revert "Use github runners for unit tests (for now)"" by @ellie in https://github.com/atuinsh/atuin/pull/1295
* Update key-binding.md by @AtomicRobotMan0101 in https://github.com/atuinsh/atuin/pull/1291
* Add commands to print the default configuration by @tobiasge in https://github.com/atuinsh/atuin/pull/1241
* Bump debian from bullseye-20230919-slim to bullseye-20231009-slim by @dependabot in https://github.com/atuinsh/atuin/pull/1304
* Bump semver from 1.0.18 to 1.0.20 by @dependabot in https://github.com/atuinsh/atuin/pull/1299
* Bump lukemathwalker/cargo-chef from latest-rust-1.72.0-buster to latest-rust-1.73.0-buster by @dependabot in https://github.com/atuinsh/atuin/pull/1297
* Bump @babel/traverse from 7.21.2 to 7.23.2 in /docs by @dependabot in https://github.com/atuinsh/atuin/pull/1309
* Switch to Actuated for docker builds by @ellie in https://github.com/atuinsh/atuin/pull/1312
* use the short sha to tag images by @ellie in https://github.com/atuinsh/atuin/pull/1313
* Checkout repo so the manifest publish step can read git by @ellie in https://github.com/atuinsh/atuin/pull/1314
* Add enter_accept to immediately execute an accepted command by @ellie in https://github.com/atuinsh/atuin/pull/1311
* Add fish support for `enter_accept` by @ellie in https://github.com/atuinsh/atuin/pull/1315
* allow binding server to hostname by @conradludgate in https://github.com/atuinsh/atuin/pull/1318
* Add bash support to `enter_accept` by @ellie in https://github.com/atuinsh/atuin/pull/1316
* Document that the self-hosted port is TCP by @Nemo157 in https://github.com/atuinsh/atuin/pull/1317
* fix(nushell): Ignore stderr messages by @arcuru in https://github.com/atuinsh/atuin/pull/1320
* Revert "Revert "Revert "Use github runners for unit tests (for now)""" by @ellie in https://github.com/atuinsh/atuin/pull/1325
* Correct some secrets filter regex by @ellie in https://github.com/atuinsh/atuin/pull/1326
* Prepare release v17.0.0 by @ellie in https://github.com/atuinsh/atuin/pull/1327
* Fix deleted history count by @ellie in https://github.com/atuinsh/atuin/pull/1328
* fix(nu): disable the up-arrow keybinding for Nushell by @arcuru in https://github.com/atuinsh/atuin/pull/1329

## New Contributors
* @thePanz made their first contribution in https://github.com/atuinsh/atuin/pull/1157
* @tobiasge made their first contribution in https://github.com/atuinsh/atuin/pull/1167
* @tombh made their first contribution in https://github.com/atuinsh/atuin/pull/1174
* @edwardloveall made their first contribution in https://github.com/atuinsh/atuin/pull/1173
* @jinnko made their first contribution in https://github.com/atuinsh/atuin/pull/1131
* @Artanicus made their first contribution in https://github.com/atuinsh/atuin/pull/1205
* @mentalisttraceur made their first contribution in https://github.com/atuinsh/atuin/pull/1200
* @mateuscomh made their first contribution in https://github.com/atuinsh/atuin/pull/1226
* @deicon made their first contribution in https://github.com/atuinsh/atuin/pull/1234
* @kiran-4444 made their first contribution in https://github.com/atuinsh/atuin/pull/1252
* @dcarosone made their first contribution in https://github.com/atuinsh/atuin/pull/1270
* @rriski made their first contribution in https://github.com/atuinsh/atuin/pull/1276
* @offbyone made their first contribution in https://github.com/atuinsh/atuin/pull/1274
* @AtomicRobotMan0101 made their first contribution in https://github.com/atuinsh/atuin/pull/1291
* @Nemo157 made their first contribution in https://github.com/atuinsh/atuin/pull/1317

**Full Changelog**: https://github.com/atuinsh/atuin/compare/v16.0.0...v17.0.0
