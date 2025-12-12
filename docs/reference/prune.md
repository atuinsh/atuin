# history prune

## `atuin history prune`

This command deletes history entries matching the [history_filter](../configuration/config.md#history_filter) configuration parameter.

It can be run with `--dry-run` first to list history entries that will be removed.

| Argument         | Description                                                        |
|------------------|--------------------------------------------------------------------|
| `--dry-run`/`-n` | List matching history lines without performing the actual deletion |
