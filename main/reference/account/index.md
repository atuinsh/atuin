# account

## `atuin account`

Manage your sync account. Registering and logging in are covered as a walkthrough in [Setting up Sync](https://docs.atuin.sh/guide/sync/index.md); this page is the command reference.

`atuin register`, `atuin login`, and `atuin logout` are shorthand for the corresponding `atuin account` subcommands.

## Subcommands

### `atuin account register`

Create an account on the configured sync server.

```
atuin account register -u <USERNAME> -e <EMAIL> -p <PASSWORD>
```

| Flag              | Description                                                            |
| ----------------- | ---------------------------------------------------------------------- |
| `--username`/`-u` | Your desired username. Must be unique, alphanumerics and hyphens only  |
| `--email`/`-e`    | Used only for important notifications, such as security issues         |
| `--password`/`-p` | Omit this to be prompted instead, keeping it out of your shell history |

Registering logs you in and generates your encryption key. Save the key — see [`atuin key`](https://docs.atuin.sh/reference/sync/#key).

### `atuin account login`

Log in on another machine.

```
atuin account login -u <USERNAME>
```

| Flag               | Description                                                  |
| ------------------ | ------------------------------------------------------------ |
| `--username`/`-u`  | Your username                                                |
| `--password`/`-p`  | Omit to be prompted                                          |
| `--key`/`-k`       | Your encryption key. Omit to be prompted                     |
| `--totp-code`/`-t` | Your two-factor authentication code, if your account has 2FA |

Omitting `--password` and `--key` is recommended: you'll be prompted for both, so neither ends up in your shell history.

### `atuin account logout`

```
atuin account logout
```

Ends the local session. Your history and encryption key stay on the machine.

### `atuin account change-password`

```
atuin account change-password
```

| Flag                      | Description                                                  |
| ------------------------- | ------------------------------------------------------------ |
| `--current-password`/`-c` | Omit to be prompted                                          |
| `--new-password`/`-n`     | Omit to be prompted                                          |
| `--totp-code`/`-t`        | Your two-factor authentication code, if your account has 2FA |

This changes your account password only. Your encryption key is unaffected, and other machines stay logged in.

### `atuin account delete`

```
atuin account delete
```

Deletes your account and all synchronized history from the server.

| Flag               | Description                                                  |
| ------------------ | ------------------------------------------------------------ |
| `--password`/`-p`  | Your password. Omit to be prompted                           |
| `--totp-code`/`-t` | Your two-factor authentication code, if your account has 2FA |

Warning

This does not prompt for confirmation, and it cannot be undone. Your local history is not affected — only the server copy.

### `atuin account link`

Link your CLI sync account to your [Atuin Hub](https://hub.atuin.sh/) account.

```
atuin account link
```

Opens a browser to authenticate with Hub, then associates the two accounts. If you are already signed in to Hub, the accounts are linked immediately.
