# Using a self hosted server

Warning

If you're self hosting, we strongly suggest you stick to tagged releases, and don't follow `main` or `latest`

Follow the GitHub releases, and please read the notes for each release. Most of the time, upgrades can occur without any manual intervention.

We can't guarantee that all updates will apply cleanly, and some may require some extra steps.

## Client setup

To use a self hosted server with Atuin, point Atuin at your server:

```
atuin setup --only --sync https://your.server
```

Setup sets `sync_address` in the config file at `~/.atuin/config.toml`. If this machine is logged in to another server, setup logs it out. See the [config](https://docs.atuin.sh/configuration/config/#sync_address) page for more details on `sync_address`.

Alternatively you can set the environment variable `ATUIN_SYNC_ADDRESS` to the correct host, for example `ATUIN_SYNC_ADDRESS=https://api.atuin.sh`.

Then create an account on your server, or log in to the account you already have there:

```
# Create a new account
atuin register -u <USERNAME> -e <EMAIL>

# Log in to an existing account
atuin login -u <USERNAME>
```

Both commands ask for your password. `atuin login` also asks for your encryption key, which `atuin key` prints on a machine that's already logged in. See [`atuin account`](https://docs.atuin.sh/reference/account/index.md) for every flag.
