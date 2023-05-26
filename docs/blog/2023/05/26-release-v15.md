---
title: Atuin v15 - Fixes and improvements release
description: Release notes for Atuin v15! 
slug: release-v15
authors: [ellie]
tags: [release]
---

Announcing a new release of Atuin! v15 is out now. This release is not particularly feature-heavy, instead we have focused on a number of bugfixes and improvements - with lots of new shiny things planned for v16.

I've also included the changes from v14.0.1 in these notes, as we never did a separate post for them

### Community

- [Discord](https://discord.gg/Fq8bJSKPHh)
- [Mastodon](https://hachyderm.io/@atuin)
- [Twitter](https://twitter.com/atuinsh)

## Sync changes

For the first time in a long while, we have made an adjustment to how sync functions. In the longer term, we intend on replacing our current sync algorithm with something that better handles consistency, but v15 should at least ship some performance improvements.

Older versions of Atuin used a fixed page size of 100. This meant that for each request, we could only upload or download 100 history items at a time. For larger histories, this meant a lot of HTTP requests + a fairly slow sync.

Atuin v15 ships a variable page size, defaulting to 1100. This is configurable on the server, via the `page_size` parameter. A smaller number of larger requests generally performs better in our testing.

For self hosted servers, please note that reverse proxies may require configuration changes to allow for larger requests.

## What's Changed
* Fix deleting history that doesn't exist yet by @ellie in #844
* Updated client config docs by @cyqsimon in #839
* Handle empty lines when importing from Bash by @cyqsimon in #845
* update str substring usage to use range parameter by @WindSoilder in #840
* Fix --delete description by @SuperSandro2000 in #853
* Use XDG data directory for fish import by @ijanos in #851
* Add some emacs movement keys by @majutsushi in #857
* Atuin stats with day, month, week and year filter by @bahdotsh in #858
* Add --reverse to atuin search by @takac in #862
* Add additional detail to search documentation by @briankung in #860
* Switch to uuidv7 by @ellie in #864
* Workspace reorder by @utter-step in #868
* Improve error message for issue #850. by @postmath in #876
* Avoid accidentally deleting all history, but allow it if intended by @ellie in #878
* Add footer by @ellie in #879
* Make the homepage prettier by @ellie in #880
* Release v14.0.1 by @ellie in #883
* Fix release workflow by @ellie in https://github.com/ellie/atuin/pull/885
* Add workflow dispatch for release by @ellie in https://github.com/ellie/atuin/pull/888
* chore: uuhhhhhh crypto lol by @conradludgate in https://github.com/ellie/atuin/pull/805
* Add keyboard shortcuts to the Config/Keybinding chapter. by @maxim-uvarov in https://github.com/ellie/atuin/pull/875
* Re-added package name to workspace.package by @bdavj in https://github.com/ellie/atuin/pull/894
* Add package param to cargo deb by @ellie in https://github.com/ellie/atuin/pull/895
* Allow specifying tag to build for workflow_dispatch by @ellie in https://github.com/ellie/atuin/pull/896
* Add symlink by @ellie in https://github.com/ellie/atuin/pull/897
* Upload tar before building deb by @ellie in https://github.com/ellie/atuin/pull/898
* Copy license for cargo-deb by @ellie in https://github.com/ellie/atuin/pull/901
* Fix fig plugin link by @millette in https://github.com/ellie/atuin/pull/924
* fix broken pipe on history list by @conradludgate in https://github.com/ellie/atuin/pull/927
* docs: Fix broken links in README.md by @xqm32 in https://github.com/ellie/atuin/pull/920
* Add `nu` section to `keybinds.md` by @VuiMuich in https://github.com/ellie/atuin/pull/881
* cwd_filter: much like history_filter, only it applies to cwd by @kjetijor in https://github.com/ellie/atuin/pull/904
* Add command flag for `inline_height` by @VuiMuich in https://github.com/ellie/atuin/pull/905
* docs: fix "From source" `cd` command by @rigrig in https://github.com/ellie/atuin/pull/937
* Correct typos in website by @skx in https://github.com/ellie/atuin/pull/946
* website: Fix participle "be ran" -> "be run" by @nh2 in https://github.com/ellie/atuin/pull/939
* Update README.md: Disable update check for offline mode by @sashkab in https://github.com/ellie/atuin/pull/960
* Bump debian from bullseye-20230320-slim to bullseye-20230502-slim by @dependabot in https://github.com/ellie/atuin/pull/930
* At least patch this on the server side so we don't loop forever by @ellie in https://github.com/ellie/atuin/pull/970
* Fix key regression by @ellie in https://github.com/ellie/atuin/pull/974
* Include bash preexec warning by @ellie in https://github.com/ellie/atuin/pull/983
* feat: add delete account option (attempt 2) by @yannickulrich in https://github.com/ellie/atuin/pull/980
* validate usernames on registration by @conradludgate in https://github.com/ellie/atuin/pull/982
* Restructure account commands to account subcommand by @ellie in https://github.com/ellie/atuin/pull/984
* Allow server configured page size by @ellie in https://github.com/ellie/atuin/pull/994
* Input bar at the top if we are in inline mode by @ellie in https://github.com/ellie/atuin/pull/866
* Add option to completely disable help row by @happenslol in https://github.com/ellie/atuin/pull/993
* Fix typo in `config.toml` by @pmodin in https://github.com/ellie/atuin/pull/1006

## New Contributors
* @WindSoilder made their first contribution in https://github.com/ellie/atuin/pull/840
* @ijanos made their first contribution in https://github.com/ellie/atuin/pull/851
* @majutsushi made their first contribution in https://github.com/ellie/atuin/pull/857
* @bahdotsh made their first contribution in https://github.com/ellie/atuin/pull/858
* @briankung made their first contribution in https://github.com/ellie/atuin/pull/860
* @utter-step made their first contribution in https://github.com/ellie/atuin/pull/868
* @postmath made their first contribution in https://github.com/ellie/atuin/pull/876
* @maxim-uvarov made their first contribution in https://github.com/ellie/atuin/pull/875
* @bdavj made their first contribution in https://github.com/ellie/atuin/pull/894
* @millette made their first contribution in https://github.com/ellie/atuin/pull/924
* @xqm32 made their first contribution in https://github.com/ellie/atuin/pull/920
* @VuiMuich made their first contribution in https://github.com/ellie/atuin/pull/881
* @kjetijor made their first contribution in https://github.com/ellie/atuin/pull/904
* @rigrig made their first contribution in https://github.com/ellie/atuin/pull/937
* @skx made their first contribution in https://github.com/ellie/atuin/pull/946
* @nh2 made their first contribution in https://github.com/ellie/atuin/pull/939
* @sashkab made their first contribution in https://github.com/ellie/atuin/pull/960
* @yannickulrich made their first contribution in https://github.com/ellie/atuin/pull/980
* @happenslol made their first contribution in https://github.com/ellie/atuin/pull/993
* @pmodin made their first contribution in https://github.com/ellie/atuin/pull/1006

**Full Changelog**: https://github.com/ellie/atuin/compare/v14.0.0...v15.0.1
