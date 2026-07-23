# store

## `atuin store`

Atuin keeps your history in an encrypted, append-only **record store**. Sync works by exchanging these records, rather than by copying rows between databases. `atuin store` inspects and repairs that store.

Most people never need these commands. Reach for them when sync is misbehaving, or when a machine ends up holding records encrypted with a key it no longer has.

Danger

`rekey`, `purge`, and the `--force` forms of `push`/`pull` can destroy data that cannot be recovered. Read the description of a command before running it, and make sure you know which key Atuin is currently using.

## Subcommands

### `atuin store status`

Print the current state of the record store — how many records exist locally, per tag and per host.

```
atuin store status
```

Start here when diagnosing a sync problem.

### `atuin store verify`

Check that every local record can be decrypted with your current key.

```
atuin store verify
```

Failures mean some records were written with a different key — usually because a machine was logged in with an old key, or a key was regenerated.

### `atuin store purge`

Delete the local records that fail decryption.

```
atuin store purge
```

Warning

This only touches the local record store on the current machine. It does not wipe your history, delete your sync account, or affect other machines.

Run `atuin store verify` first, so you know you are clearing up a genuine key mismatch rather than deleting data blindly. See [Deleting History](https://docs.atuin.sh/guide/delete-history/#purging-undecryptable-local-store-records) for the full procedure.

### `atuin store rekey [KEY]`

Re-encrypt the entire local store with a new key. Omit the key to have one generated for you.

```
atuin store rekey
```

Danger

Every other machine will need the new key before it can read anything you sync after this point. Save the new key somewhere safe first.

### `atuin store rebuild <TAG>`

Rebuild derived state from the record store — for example, regenerate the history database from `history` records.

```
atuin store rebuild history
```

Useful when the record store is intact but the local view of it isn't.

### `atuin store push`

Upload local records to the sync server, one way.

| Flag         | Description                                                                  |
| ------------ | ---------------------------------------------------------------------------- |
| `--tag`/`-t` | Only push this tag (e.g. `history`). Defaults to all tags                    |
| `--host`     | Only push this host, given as a host UUID. Defaults to the current host      |
| `--force`    | Clear the remote store, then upload everything local, for all hosts and tags |
| `--page`     | How many records to upload at a time (default: 100)                          |

### `atuin store pull`

Download records from the sync server, one way.

| Flag         | Description                                                          |
| ------------ | -------------------------------------------------------------------- |
| `--tag`/`-t` | Only pull this tag. Defaults to all tags                             |
| `--force`    | Wipe the local store first, then download everything from the remote |
| `--page`     | How many records to download at a time (default: 100)                |

For ordinary two-way syncing, use [`atuin sync`](https://docs.atuin.sh/reference/sync/index.md) instead.
