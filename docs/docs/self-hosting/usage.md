# Using a self hosted server

!!! warning
    If you are self hosting, we strongly suggest you stick to tagged releases, and do not follow `main` or `latest`

    Follow the GitHub releases, and please read the notes for each release. Most of the time, upgrades can occur without any manual intervention.

    We cannot guarantee that all updates will apply cleanly, and some may require some extra steps.

## Client setup

In order use a self hosted server with Atuin, you'll have to set up the `sync_address` in the config file at `~/.config/atuin/config.toml`. See the [config](../configuration/config.md#sync_address) page for more details on how to set the `sync_address`.

Alternatively you can set the environment variable `ATUIN_SYNC_ADDRESS` to the correct host ie.: `ATUIN_SYNC_ADDRESS=https://api.atuin.sh`.
