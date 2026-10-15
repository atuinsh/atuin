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

The server's config lives at `~/.atuin/server.toml`, separate from the client's config.

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

| Parameter           | Description                                                                |
| ------------------- | -------------------------------------------------------------------------- |
| `db_uri`            | A valid database URI, for saving history (required, no default)            |
| `db_password_file`  | A file holding the database password, kept out of `db_uri` (default: none) |
| `host`              | The host to listen on (default: 127.0.0.1)                                 |
| `port`              | The TCP port to listen on (default: 8888)                                  |
| `open_registration` | If `true`, accept new user registrations (default: false)                  |
| `path`              | A path to prepend to all routes of the server (default: empty)             |

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

### Keeping the password in a file

Instead of writing the password into `db_uri`, you can keep it in a file, such as a [Docker](https://docs.atuin.sh/self-hosting/docker/#keeping-the-password-in-a-secret) or [Kubernetes](https://docs.atuin.sh/self-hosting/kubernetes/#keeping-the-password-in-a-secret) secret, and point `db_password_file` at it:

```
db_uri="postgres://user@hostname/database"
db_password_file="/run/secrets/atuin_db_password"
```

Or with environment variables:

```
ATUIN_DB_URI="postgres://user@hostname/database"
ATUIN_DB_PASSWORD_FILE="/run/secrets/atuin_db_password"
```

Trailing newlines in the file are ignored, and the rest is used as-is, so unlike a password in `db_uri` it doesn't need escaping. `db_uri` must name the host, and must not contain a password as well. This works with PostgreSQL and MySQL; SQLite has no password.

## TLS

We strongly urge you enable TLS with Atuin. Without TLS, passwords are sent plaintext, which is wildly insecure.

For TLS/HTTPS support, we recommend using a reverse proxy such as [nginx](https://nginx.org/), [Caddy](https://caddyserver.com/), or [Traefik](https://traefik.io/traefik) in front of the Atuin server.
