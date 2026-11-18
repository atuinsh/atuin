# sync

Atuin can back up your history to a server, and use this to make sure multiple
machines have the same shell history. This is all encrypted end-to-end, so the
server operator can _never_ see your data!

Anyone can host a server (see the [self-hosting docs](../self-hosting/server-setup.md)),
and we host one at <https://api.atuin.sh>. This is the default server address,
which can be changed in the [config](../configuration/config.md#sync_address).
Again, we _can't_ see your data, and don't want to.

To sync with your own server, run `atuin setup --only --sync <URL>`, which sets
that address.

## Sync frequency

Syncing will happen automatically, unless configured otherwise. The sync
frequency is configurable in [config](../configuration/config.md#sync_frequency)

## Sync

You can manually trigger a sync with `atuin sync`

## Register

`atuin setup --sync cloud` signs this machine in to Atuin Cloud through your
browser. To register on a server of your own, or from a script, run:

```shell
atuin register -u <USERNAME> -e <EMAIL> -p <PASSWORD>
```

If you don't want to have your password be included in shell history, you can omit
the password flag and Atuin asks for it through `stdin`.

Usernames must be unique and only contain alphanumerics or hyphens,
and emails will only be used for important notifications (security breaches, changes to service, etc).

Upon success, you're also logged in :) Syncing should happen automatically from
here!

## Delete

You can delete your sync account with

```shell
atuin account delete
```

This will remove your account and all synchronized history from the server. Local data won't be touched!

## Key

As all your data is encrypted, Atuin generates a key for you. It's stored in the
Atuin data directory (`~/.atuin/data` by default).

You can also get this with

```shell
atuin key
```

Never share this with anyone!

## Login

On a new machine, `atuin setup --sync cloud` signs in to Atuin Cloud through
your browser. Setup asks for your encryption key only when your account has
data that this machine's key can't read.

To log in to a server of your own, you need your encryption key. `atuin key`
prints it on a machine that's already logged in. For unattended logins, see
[`atuin account login`](account.md#atuin-account-login).

```shell
atuin login -u <USERNAME> -p <PASSWORD> -k <KEY>
```

If you don't want to have your password or encryption key be included in shell history, you can omit
the corresponding flag and Atuin asks for it through `stdin`.

## Logout

```shell
atuin logout
```
