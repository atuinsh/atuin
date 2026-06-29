# Advanced Usage

Atuin offers you several options to help navigate through the results.

## Filter mode

The command history can be filtered in different ways, letting you narrow the search scope.

You can cycle through the different modes by pressing **ctrl-r**.

The available modes are:

| Mode             | Description                                                                          |
|------------------|--------------------------------------------------------------------------------------|
| global (default) | Search from the full history                                                         |
| host             | Search history from this host                                                        |
| session          | Search history from the current session                                              |
| directory        | Search history from the current directory                                            |
| workspace        | Search history from the current git repository                                       |
| session-preload  | Search from the current session and the global history from before the session start |

See the [`filter_mode` config reference](../configuration/config.md#filter_mode) for more details.

## Search mode

Atuin offers different modes to interpret your search query.

You can cycle through the different modes by pressing **ctrl-s**.

The available modes are:

| Mode            | Description                                                                                                    |
|-----------------|----------------------------------------------------------------------------------------------------------------|
| fuzzy (default) | Search for commands in a fuzzy way, similar to the [fzf syntax](https://github.com/junegunn/fzf#search-syntax) |
| prefix          | Commands that start with your query                                                                            |
| fulltext        | Commands that contain your query as a substring                                                                |
| skim            | Search for commands using the [skim syntax](https://github.com/lotabout/skim#search-syntax)                    |

See the [`search_mode` config reference](../configuration/config.md#search_mode) for more details.

## Context switch

Atuin uses the current context (host, session, directory) to filter the history when you use a filter mode other than *global*.

You can switch this context to the one of the currently selected command by pressing **ctrl-a** then **c**.

This will set the filter mode to *session* and clear the search query, which will show you all the commands executed in the same shell session.

Pressing this key combination again will return to the initial context. You can customize this behavior by setting [custom key bindings](../configuration/advanced-key-binding.md) to the `switch-context` and `clear-context` commands. `switch-context` can be called several times to navigate through multiple command contexts, while `clear-context` will always return to the initial context.
