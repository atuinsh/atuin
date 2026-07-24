# history prune

## `atuin history prune`

This command deletes history entries matching the [`history_filter`](../configuration/config.md#history_filter) configuration parameter.

These are commands that match the current `history_filter` configuration, but Atuin saved them to history before you set up the filter. Because the filter didn't exist yet, Atuin didn't exclude them from history at the time.

It may be useful to run the prune command after updating `history_filter` to remove old history entries that match the new filters.

It can be run with `--dry-run` first to list history entries that will be removed.

| Argument         | Description                                                        |
|------------------|--------------------------------------------------------------------|
| `--dry-run`/`-n` | List matching history lines without performing the actual deletion |
