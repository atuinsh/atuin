# `atuin sync`

Atuin can back up your history to a server, and use this to ensure multiple
machines have the same shell history. This is all encrypted end-to-end, so the
server operator can _never_ see your data!

Anyone can host a server (try `atuin server start`, more docs to follow), but I
host one at https://api.atuin.sh. This is the default server address, which can
be changed in the [config](/docs/config/config.md#sync_address). Again, I _cannot_ see your data, and
do not want to.

## Sync frequency

Syncing will happen automatically, unless configured otherwise. The sync
frequency is configurable in [config](/docs/config/config.md#sync_frequency)

## Sync

You can manually trigger a sync with `atuin sync`

## Register

Register for a sync account with

```
atuin register -u <USERNAME> -e <EMAIL> -p <PASSWORD>
```

Usernames must be unique, and emails shall only be used for important
notifications (security breaches, changes to service, etc).

Upon success, you are also logged in :) Syncing should happen automatically from
here!

## Unregister

You can delete your sync account with

```
atuin unregister
```

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

## Logout

```
atuin logout
```
