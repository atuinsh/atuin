# history prune

## `atuin history prune`

This command deletes history entries matching the [history_filter](../configuration/config.md#history_filter) configuration parameter.

These may be commands that match the current `history_filter` configuration, but were saved to history before the filter was set up, and therefore were not excluded from history at the time of execution.

It may be useful to run the prune command after updating `history_filter` to remove old history entries that match the new filters.

It can be run with `--dry-run` first to list history entries that will be removed.

| Argument         | Description                                                        |
|------------------|--------------------------------------------------------------------|
| `--dry-run`/`-n` | List matching history lines without performing the actual deletion |
