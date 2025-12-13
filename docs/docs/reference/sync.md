# sync

Atuin can back up your history to a server, and use this to ensure multiple
machines have the same shell history. This is all encrypted end-to-end, so the
server operator can _never_ see your data!

Anyone can host a server (try `atuin server start`, more docs to follow), but I
host one at https://api.atuin.sh. This is the default server address, which can
be changed in the [config](../configuration/config.md#sync_address). Again, I _cannot_ see your data, and
do not want to.

## Sync frequency

Syncing will happen automatically, unless configured otherwise. The sync
frequency is configurable in [config](../configuration/config.md#sync_frequency)

## Sync

You can manually trigger a sync with `atuin sync`

## Register

Register for a sync account with

```
atuin register -u <USERNAME> -e <EMAIL> -p <PASSWORD>
```

If you don't want to have your password be included in shell history, you can omit
the password flag and you will be prompted to provide it through stdin.

Usernames must be unique and only contain alphanumerics or hyphens,
and emails shall only be used for important notifications (security breaches, changes to service, etc).

Upon success, you are also logged in :) Syncing should happen automatically from
here!

## Delete

You can delete your sync account with

```
atuin account delete
```

This will remove your account and all synchronized history from the server. Local data will not be touched!

## Key

As all your data is encrypted, Atuin generates a key for you. It's stored in the
Atuin data directory (`~/.local/share/atuin` on Linux).

You can also get this with

```
atuin key
```

Never share this with anyone!

## Login

If you want to log in to a new machine, you will require your encryption key
(`atuin key`).

```
atuin login -u <USERNAME> -p <PASSWORD> -k <KEY>
```

If you don't want to have your password be included in shell history, you can omit
the password flag and you will be prompted to provide it through stdin.

## Logout

```
atuin logout
```
