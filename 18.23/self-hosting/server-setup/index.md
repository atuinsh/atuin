# Atuin Server

While we offer a hosted, encrypted and secure server at [Atuin Hub](https://hub.atuin.sh/), you may still wish to host your own Atuin server.

## Requirements

- You need to be able to run a binary or Docker container on a server.
- You must have either a PostgreSQL, MySQL or SQLite database.

## Quickstart

The server is distributed as a separate binary, `atuin-server`. Prebuilt binaries and an installer are published with every release on the [GitHub releases page](https://github.com/atuinsh/atuin/releases). You can install the latest version with:

```
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/atuinsh/atuin/releases/latest/download/atuin-server-installer.sh | sh
```

Once installed, start the server with:

```
atuin-server start
```

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

| Parameter           | Description                                                     |
| ------------------- | --------------------------------------------------------------- |
| `db_uri`            | A valid database URI, for saving history (required, no default) |
| `host`              | The host to listen on (default: 127.0.0.1)                      |
| `port`              | The TCP port to listen on (default: 8888)                       |
| `open_registration` | If `true`, accept new user registrations (default: false)       |
| `path`              | A path to prepend to all routes of the server (default: empty)  |

## Database

You **must** configure the database as Atuin can't proceed without one. You can do so either in `server.toml` or via the environment variable:

Using `server.toml`:

```
db_uri="postgres://user:password@hostname/database"
```

Using the `ATUIN_DB_URI` environment variable:

```
ATUIN_DB_URI="postgres://user:password@hostname/database"
```

Using `server.toml`:

```
db_uri="sqlite:///config/atuin.db"
```

Using the `ATUIN_DB_URI` environment variable:

```
ATUIN_DB_URI="sqlite:///config/atuin.db"
```

**Note that atuin will create this file if it does not exist.**

Using `server.toml`:

```
db_uri="mysql://user:password@hostname/database"
```

Using the `ATUIN_DB_URI` environment variable:

```
ATUIN_DB_URI="mysql://user:password@hostname/database"
```

Support

MySQL is currently considered a tier-two database meaning that, although we support it, issues are deprioritized in favor of tier-one databases -- PostgreSQL and SQLite.

## TLS

We strongly urge you enable TLS with Atuin. Without TLS, passwords are sent plaintext, which is wildly insecure.

For TLS/HTTPS support, we recommend using a reverse proxy such as [nginx](https://nginx.org/), [Caddy](https://caddyserver.com/), or [Traefik](https://traefik.io/traefik) in front of the Atuin server.
