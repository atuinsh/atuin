# Server setup

While we offer a public sync server, and cannot view your data (as it is encrypted), you may still wish to self host your own Atuin sync server.

The requirements to do so are pretty minimal! You need to be able to run a binary or docker container, and have a PostgreSQL database set up. Atuin requires PostgreSQL 14 or above.

Alternatively, the server can use SQLite (version 3 or above) instead of PostgreSQL.

The server is distributed as a separate binary, `atuin-server`. Prebuilt binaries and an installer are published with every release on the [GitHub releases page](https://github.com/atuinsh/atuin/releases). For example, to install the latest release:

```
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/atuinsh/atuin/releases/latest/download/atuin-server-installer.sh | sh
```

Once installed, start the server with:

```
atuin-server start
```

Note

Prior to v18.12.0, the server was bundled into the main `atuin` binary and started with `atuin server start`. If you are upgrading from an older release, you will need to install the new `atuin-server` binary and update any service files (systemd, docker, k8s) to invoke `atuin-server` instead of `atuin server`. See the [release notes](https://github.com/atuinsh/atuin/releases) for details.

## Configuration

The server's config lives at `~/.config/atuin/server.toml`, separate from the client's config.

It looks something like this for PostgreSQL:

```
host = "0.0.0.0"
port = 8888
open_registration = true
db_uri="postgres://user:password@hostname/database"
```

Alternatively, configuration can also be provided with environment variables.

```
ATUIN_HOST="0.0.0.0"
ATUIN_PORT=8888
ATUIN_OPEN_REGISTRATION=true
ATUIN_DB_URI="postgres://user:password@hostname/database"
```

| Parameter           | Description                                                                 |
| ------------------- | --------------------------------------------------------------------------- |
| `host`              | The host to listen on (default: 127.0.0.1)                                  |
| `port`              | The TCP port to listen on (default: 8888)                                   |
| `open_registration` | If `true`, accept new user registrations (default: false)                   |
| `db_uri`            | A valid PostgreSQL or SQLite URI, for saving history (required, no default) |
| `path`              | A path to prepend to all routes of the server (default: empty)              |

For sqlite, use the following in your server.toml:

```
db_uri="sqlite:///config/atuin.db"
```

Alternatively, provide the Database URI via an environment variable

```
ATUIN_DB_URI="sqlite:///config/atuin.db"
```

These will create the database in the `/config` directory. Be sure to map a persistent volume to the `/config` directory that is writable by the atuin server.

### TLS

For TLS/HTTPS support, we recommend using a reverse proxy such as nginx, caddy, or traefik in front of the Atuin server. This is the standard approach for containerized applications and provides better flexibility for certificate management.

> **Note:** The built-in `[tls]` configuration option has been removed. If you were previously using it, please migrate to a reverse proxy setup. Any existing `[tls]` sections in your config will be ignored.
