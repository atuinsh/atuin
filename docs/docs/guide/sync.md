# Setting up Sync

At this point, you have Atuin storing and searching your shell history! But it
isn't syncing it just yet. To do so, you'll need to register with the sync
server. All of your history is fully end-to-end encrypted, so there are no
risks of the server snooping on you.

If you don't have an account, please [register](#register). If you have already registered,
proceed to [login](#login).

**Note:** You first have to set up your `sync_address` if you want to use a [self hosted server](../self-hosting/server-setup.md).

## Register

```
atuin register -u <YOUR_USERNAME> -e <YOUR EMAIL>
```

After registration, Atuin will generate an encryption key for you and store it
locally. This is needed for logging in to other machines, and can be seen with

```
atuin key
```

Please **never** share this key with anyone! The Atuin developers will never
ask you for your key, your password, or the contents of your Atuin directory.

If you lose your key, we can do nothing to help you. We recommend you store
this somewhere safe, such as in a password manager.

## First sync
By default, Atuin will sync your history once per hour. This can be
[configured](../configuration/config.md#sync_frequency).

To run a sync manually, please run

```
atuin sync
```

Atuin tries to be smart with a sync, and not waste data transfer. However, if
you are seeing some missing data, please try running

```
atuin sync -f
```

This triggers a full sync, which may take longer as it works through historical data.

## Login

When only signed in on one machine, Atuin sync operates as a backup. This is
pretty useful by itself, but syncing multiple machines is where the magic
happens.

First, ensure you are [registered with the sync server](#register) and make a
note of your key. You can see this with `atuin key`.

Then, install Atuin on a new machine. Once installed, login with

```
atuin login -u <USERNAME>
```

You will be prompted for your password, and for your key.

Syncing will happen automatically in the background, but you may wish to run it manually with

```
atuin sync
```

Or, if you see missing data, force a full sync with:

```
atuin sync -f
```
