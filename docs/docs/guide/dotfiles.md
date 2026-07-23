# Syncing dotfiles

While Atuin started as a tool for syncing and searching shell history, we are
building tooling for syncing dotfiles across machines, and making them easier
to work with.

At the moment, we support managing and syncing of shell aliases and environment variables - with more
coming soon.

The following shells are supported:

- zsh
- bash
- fish
- xonsh
- powershell

Note: Atuin handles your configuration internally, so once it is installed you
no longer need to edit your config files manually.

## Required config

Once Atuin is set up and installed, the following is required in your config file (`~/.config/atuin/config.toml`)

```toml
[dotfiles]
enabled = true
```

In a later release, this will be enabled by default.

## Usage

### Aliases

After creating or deleting an alias, remember to restart your shell!

#### Creating an alias

```shell
atuin dotfiles alias set NAME 'COMMAND'
```

For example, to alias `k` to be `kubectl`


```shell
atuin dotfiles alias set k 'kubectl'
```

or to alias `ll` to be  `ls -lah`

```shell
atuin dotfiles alias set ll 'ls -lah'
```

#### Deleting an alias

Deleting an alias is as simple as:

```shell
atuin dotfiles alias delete NAME
```

For example, to delete the above alias `k`:

```shell
atuin dotfiles alias delete k
```

#### Listing aliases

You can list all aliases with:

```shell
atuin dotfiles alias list
```

### Env vars

After creating or deleting an env var, remember to restart your shell!

#### Creating a var

```shell
atuin dotfiles var set NAME 'value'
```

For example, to set `FOO` to be `bar`


```shell
atuin dotfiles var set FOO 'bar'
```

Vars are exported by default, but you can create a shell var like so

```shell
atuin dotfiles var set -n foo 'bar'
```


#### Deleting a var

Deleting a var is as simple as:

```shell
atuin dotfiles var delete NAME
```

For example, to delete the above var `FOO`:

```shell
atuin dotfiles var delete FOO
```

#### Listing vars

You can list all vars with:

```shell
atuin dotfiles var list
```

### Syncing and backing up dotfiles
If you have [set up sync](sync.md), then running

```shell
atuin sync
```

will back up your config to the server and sync it across machines.
