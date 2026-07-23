# Advanced Usage

Two settings shape every search you run: the **filter mode** decides *which* commands are searched, and the **search mode** decides *how* your query is matched against them. Both can be changed on the fly from inside the TUI.

## Filter mode

The filter mode narrows the set of history Atuin searches. Cycle through the modes by pressing **ctrl-r** inside the TUI.

| Mode             | Searches                                                                     |
| ---------------- | ---------------------------------------------------------------------------- |
| global (default) | Your full history, from every machine                                        |
| host             | Only history from this machine                                               |
| session          | Only history from the current shell session                                  |
| directory        | Only history from the current directory                                      |
| workspace        | Only history from anywhere in the current git repository                     |
| session-preload  | The current session, plus all global history from before the session started |

`workspace` mode requires [`workspaces = true`](https://docs.atuin.sh/configuration/config/#workspaces), and is skipped when you are not inside a git repository.

To change which mode searches start in, set [`filter_mode`](https://docs.atuin.sh/configuration/config/#filter_mode). To remove modes from the ctrl-r rotation entirely, set [`search.filters`](https://docs.atuin.sh/configuration/config/#filters). The up arrow can start in a different mode than ctrl-r — see [`filter_mode_shell_up_key_binding`](https://docs.atuin.sh/configuration/config/#filter_mode_shell_up_key_binding).

## Search mode

The search mode decides how your query text is interpreted. Cycle through the modes by pressing **ctrl-s** inside the TUI.

| Mode            | Matches                                                                                                                                                                     |
| --------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| fuzzy (default) | Fuzzily, using the [fzf syntax](https://github.com/junegunn/fzf#search-syntax) — see [fuzzy search syntax](https://docs.atuin.sh/configuration/config/#fuzzy-search-syntax) |
| prefix          | Commands that start with your query                                                                                                                                         |
| fulltext        | Commands that contain your query anywhere                                                                                                                                   |
| skim            | Using the [skim syntax](https://github.com/lotabout/skim#search-syntax)                                                                                                     |
| daemon-fuzzy    | Like `fuzzy`, but served from the [daemon's](https://docs.atuin.sh/reference/daemon/index.md) in-memory index, with tunable scoring                                         |

To change the default, set [`search_mode`](https://docs.atuin.sh/configuration/config/#search_mode).

## Context switch

Atuin uses the current context (host, session, directory) to filter the history when you use a filter mode other than *global*.

You can switch this context to the one of the currently selected command by pressing **ctrl-a** then **c**.

This will set the filter mode to *session* and clear the search query, which will show you all the commands executed in the same shell session.

Pressing this key combination again will return to the initial context. You can customize this behavior by setting [custom key bindings](https://docs.atuin.sh/configuration/advanced-key-binding/index.md) to the `switch-context` and `clear-context` commands. `switch-context` can be called several times to navigate through multiple command contexts, while `clear-context` will always return to the initial context.
