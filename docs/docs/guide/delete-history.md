# Deleting History

Atuin provides several ways to delete history, whether you want to remove a single entry, bulk delete by query, clean up duplicates, or wipe everything.

All deletion methods are local-first. If you have sync enabled, deletions are propagated to other machines automatically.

## Deleting a single entry

The quickest way to delete a single entry is via the interactive TUI.

### Using the inspector

1. Open the TUI with ++ctrl+r++ or the up arrow
2. Search for the entry you want to delete
3. Press ++ctrl+o++ to open the inspector on the selected entry
4. Verify this is the correct entry
5. Press ++ctrl+d++ to delete it

### Using the prefix shortcut

1. Open the TUI with ++ctrl+r++ or the up arrow
2. Navigate to the entry you want to delete
3. Press ++ctrl+a++ then ++d++ to delete the selected entry

Both methods remove the entry immediately with no further confirmation.

## Deleting entries matching a query

Use `atuin search --delete` to delete all entries matching a search query. This uses the same query syntax as regular search, so you can preview what will be deleted before committing.

### Preview first, then delete

Always run your query without `--delete` first to verify the results:

```
# Step 1: preview - see what matches
atuin search "^curl https://internal"

# Step 2: delete - once you're satisfied the results are correct
atuin search --delete "^curl https://internal"
```

### Combining filters

You can combine `--delete` with any search filter:

```
# Delete all failed commands run from a specific directory
atuin search --delete --exit 1 --cwd /home/user/experiments

# Delete commands matching a pattern that ran before a certain date
atuin search --delete --before "2024-01-01" "^tmp-script"

# Delete successful cargo commands run after yesterday at 3pm
atuin search --delete --exit 0 --after "yesterday 3pm" cargo
```

!!! warning
    `--delete` requires a query or filter. It will not run without one. This is intentional to prevent accidental bulk deletion.

## Deleting all history

If you want to wipe your entire local history:

```
atuin search --delete-it-all
```

!!! danger
    This deletes every entry in your local history database. It cannot be combined with a query or filters. This action is irreversible.

### Starting fresh with sync

If you use sync and want to start completely fresh, `--delete-it-all` alone is not enough. Atuin sync works by recording every action (including deletions) as encrypted records. Deleting 100,000 entries locally creates 100,000 delete records that still need to sync. When your other machines pull those records, they process every single one, and your database still contains the overhead of all that history.

The cleaner approach is to delete your sync account and start over:

```
# Delete your sync account and all server-side data
atuin account delete

# Register a new account
atuin register

# Import your shell history fresh (optional)
atuin import auto
```

This gives you a clean slate on the server with no leftover records. Your other machines can then register with the new account and start fresh too.

!!! tip
    If you only want to delete specific entries and keep the rest, `atuin search --delete` is the right tool. The account reset approach is only better when you want to wipe everything and start over.

## Pruning filtered commands

If you've updated your [`history_filter`](../configuration/config.md#history_filter) config and want to retroactively remove entries that match the new filters:

```
# Preview what will be removed
atuin history prune --dry-run

# Perform the deletion
atuin history prune
```

This is useful when you add a new pattern to `history_filter` - future commands matching the filter are never recorded, but old entries that were recorded before the filter was set up remain. `prune` cleans those up.

## Deduplicating history

Remove duplicate entries (same command, working directory, and hostname):

```
# Preview duplicates that would be removed
atuin history dedup --dry-run --before "2025-01-01" --dupkeep 1

# Delete them
atuin history dedup --before "2025-01-01" --dupkeep 1
```

| Flag | Description |
|------|-------------|
| `--dry-run`/`-n` | List duplicates without deleting |
| `--before`/`-b` | Only consider entries added before this date (required) |
| `--dupkeep` | Number of recent duplicates to keep |

## Deleting your sync account

To delete your remote sync account and all server-side history:

```
atuin account delete
```

This removes your account and all synchronized history from the server. **Local history is not affected.** See the [sync reference](../reference/sync.md) for more details.

## Summary

| Goal | Command |
|------|---------|
| Delete one entry (TUI) | ++ctrl+o++ then ++ctrl+d++, or ++ctrl+a++ then ++d++ |
| Delete entries by query | `atuin search --delete <query>` |
| Delete all history | `atuin search --delete-it-all` |
| Start fresh (with sync) | `atuin account delete` then re-register |
| Remove filtered entries | `atuin history prune` |
| Remove duplicates | `atuin history dedup --before <date> --dupkeep <n>` |
| Delete sync account | `atuin account delete` |
