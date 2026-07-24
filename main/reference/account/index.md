# account

## `atuin account`

Manage your sync account. This page is the command reference. For a walkthrough of registering and logging in, see [Setting up Sync](https://docs.atuin.sh/guide/sync/index.md).

`atuin register`, `atuin login`, and `atuin logout` are shorthand for the corresponding `atuin account` subcommands.

## Subcommands

### `atuin account register`

Create an account on the configured sync server.

```
atuin account register -u <USERNAME> -e <EMAIL> -p <PASSWORD>
```

| Flag              | Description                                                                   |
| ----------------- | ----------------------------------------------------------------------------- |
| `--username`/`-u` | Your desired username. Must be unique, alphanumerics and hyphens only         |
| `--email`/`-e`    | Used only for important notifications, such as security issues                |
| `--password`/`-p` | Omit this and Atuin asks for it instead, keeping it out of your shell history |

Registering logs you in and generates your encryption key. Save the key — see [`atuin key`](https://docs.atuin.sh/reference/sync/#key).

### `atuin account login`

Log in on another machine.

```
atuin account login -u <USERNAME>
```

| Flag               | Description                                                  |
| ------------------ | ------------------------------------------------------------ |
| `--username`/`-u`  | Your username                                                |
| `--password`/`-p`  | Omit and Atuin asks for it                                   |
| `--key`/`-k`       | Your encryption key. Omit and Atuin asks for it              |
| `--totp-code`/`-t` | Your two-factor authentication code, if your account has 2FA |

Omitting `--password` and `--key` is recommended: Atuin asks for both, so neither ends up in your shell history.

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
| `--current-password`/`-c` | Omit and Atuin asks for it                                   |
| `--new-password`/`-n`     | Omit and Atuin asks for it                                   |
| `--totp-code`/`-t`        | Your two-factor authentication code, if your account has 2FA |

This changes your account password only. Your encryption key is unaffected, and other machines stay logged in.

### `atuin account delete`

```
atuin account delete
```

Deletes your account and all synchronized history from the server.

| Flag               | Description                                                  |
| ------------------ | ------------------------------------------------------------ |
| `--password`/`-p`  | Your password. Omit and Atuin asks for it                    |
| `--totp-code`/`-t` | Your two-factor authentication code, if your account has 2FA |

Warning

This doesn't prompt for confirmation, and it can't be undone. Your local history isn't affected — only the server copy.

### `atuin account link`

Link your CLI sync account to your [Atuin Hub](https://hub.atuin.sh/) account.

```
atuin account link
```

Opens a browser to authenticate with Hub, then associates the two accounts. If you're already signed in to Hub, the accounts are linked immediately.
