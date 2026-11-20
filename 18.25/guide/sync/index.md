# Setting up Sync

At this point, you have Atuin storing and searching your shell history! To sync it between machines, you'll need an account on the sync server. All of your history is fully end-to-end encrypted, so there are no risks of the server snooping on you.

`atuin setup` offers to sign you in to Atuin Cloud when it sets Atuin up. If you skipped sync and don't have an account yet, please [register](#register). If you have already registered, proceed to [login](#login).

**Note:** To use a [self hosted server](https://docs.atuin.sh/self-hosting/server-setup/index.md), first set your `sync_address` with setup:

```
atuin setup --only --sync https://your.server
```

Setup writes the address to your config. Then register or log in with the self-hosted commands below.

## Register

To create an Atuin Cloud account, run:

```
atuin setup --sync cloud
```

Setup opens your browser, where you can create an account or sign in. Because sign-in needs the browser, `atuin setup --yes` and `atuin setup --only` can't sign you in to Atuin Cloud.

On a self-hosted server, register with:

```
atuin register -u <YOUR_USERNAME> -e <YOUR EMAIL>
```

Atuin generates an encryption key for you and stores it locally. This is needed for logging in to other machines, and can be seen with

```
atuin key
```

Please **never** share this key with anyone! The Atuin developers will never ask you for your key, your password, or the contents of your Atuin directory.

If you lose your key, we can do nothing to help you. We recommend you store this somewhere safe, such as in a password manager.

## First sync

By default, Atuin will sync your history once per hour. This can be [configured](https://docs.atuin.sh/configuration/config/#sync_frequency).

To run a sync manually, please run

```
atuin sync
```

Atuin tries to be smart with a sync, and not waste data transfer. However, if you're seeing some missing data, please try running

```
atuin sync -f
```

This triggers a full sync, which may take longer as it works through historical data.

## Login

When only signed in on one machine, Atuin sync operates as a backup. This is pretty useful by itself, but syncing multiple machines is where the magic happens.

First, make sure you're [registered with the sync server](#register) and make a note of your key. You can see this with `atuin key`.

Then, install Atuin on a new machine. The install script runs `atuin setup`, which offers to sign you in. If you skipped sync during setup, or installed Atuin another way, sign in with

```
atuin setup --sync cloud
```

Setup opens your browser to sign in. If your account has data that this machine's key can't read, setup asks for your key.

On a self-hosted server, log in with

```
atuin login -u <USERNAME>
```

Atuin asks for your password, and for your key.

Syncing will happen automatically in the background, but you may wish to run it manually with

```
atuin sync
```

Or, if you see missing data, force a full sync with:

```
atuin sync -f
```
