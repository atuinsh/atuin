# Atuin Server

While we offer a hosted, encrypted and secure server at [Atuin
Hub](https://hub.atuin.sh/), you may still wish to host your own Atuin server.

## Requirements

- You need to be able to run a binary or Docker container on a server.
- You must have either a PostgreSQL, MySQL or SQLite database.

## Quickstart

The server is distributed as a separate binary, `atuin-server`. Prebuilt
binaries and an installer are published with every release on the [GitHub
releases page](https://github.com/atuinsh/atuin/releases). You can install the
latest version with:

```shell
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/atuinsh/atuin/releases/latest/download/atuin-server-installer.sh | sh
```

Once installed, start the server with:

```shell
atuin-server start
```

## Configuration

The server's config lives at `~/.atuin/server.toml`, separate from the client's
config.

It looks something like this for PostgreSQL:

```toml
host = "0.0.0.0"
port = 8888
open_registration = true
db_uri="postgres://user:password@hostname/database"
```

Alternatively, configuration can also be provided with environment variables.

```sh
ATUIN_HOST="0.0.0.0"
ATUIN_PORT=8888
ATUIN_OPEN_REGISTRATION=true
ATUIN_DB_URI="postgres://user:password@hostname/database"
```

An `ATUIN_*` environment variable overrides the same key in `server.toml`. An
empty one, such as `ATUIN_DB_URI=` left in a compose file, counts as unset.
`ATUIN_PORT` in the `tcp://10.96.0.12:8888` form, which Kubernetes sets for a
Service named `atuin`, is ignored with a warning; see
[Kubernetes](kubernetes.md#service-links).

| Parameter              | Description                                                                        |
| ---------------------- | ---------------------------------------------------------------------------------- |
| `db_uri`               | A valid database URI, for saving history (required, no default)                    |
| `db_password_file`     | A file holding the database password, kept out of `db_uri` (default: none)         |
| `host`                 | The host to listen on (default: 127.0.0.1)                                         |
| `port`                 | The TCP port to listen on (default: 8888)                                          |
| `open_registration`    | If `true`, accept new user registrations (default: false)                          |
| `path`                 | A path to prepend to all routes of the server (default: empty)                     |
| `trusted_proxies`      | Reverse proxies whose client address header is trusted (default: empty)            |
| `trusted_proxy_header` | The header those proxies report the client address in (default: `X-Forwarded-For`) |

## Database

You **must** configure the database as Atuin can't proceed without one. You
can do so either in `server.toml` or via the environment variable:

=== "Postgres"

    Using `server.toml`:

    ```toml
    db_uri="postgres://user:password@hostname/database"
    ```

    Using the `ATUIN_DB_URI` environment variable:

    ```bash
    ATUIN_DB_URI="postgres://user:password@hostname/database"
    ```

=== "SQLite"

    Using `server.toml`:

    ```toml
    db_uri="sqlite:///config/atuin.db"
    ```

    Using the `ATUIN_DB_URI` environment variable:

    ```bash
    ATUIN_DB_URI="sqlite:///config/atuin.db"
    ```

    **Note that atuin will create this file if it does not exist.**

=== "MySQL"

    Using `server.toml`:

    ```toml
    db_uri="mysql://user:password@hostname/database"
    ```

    Using the `ATUIN_DB_URI` environment variable:

    ```bash
    ATUIN_DB_URI="mysql://user:password@hostname/database"
    ```

    !!! warn "Support"

        MySQL is currently considered a tier-two database meaning that,
        although we support it, issues are deprioritized in favor of tier-one
        databases -- PostgreSQL and SQLite.

### Keeping the password in a file

Instead of writing the password into `db_uri`, you can keep it in a file, such
as a [Docker](docker.md#keeping-the-password-in-a-secret) or
[Kubernetes](kubernetes.md#keeping-the-password-in-a-secret) secret, and point
`db_password_file` at it:

```toml
db_uri="postgres://user@hostname/database"
db_password_file="/run/secrets/atuin_db_password"
```

Or with environment variables:

```bash
ATUIN_DB_URI="postgres://user@hostname/database"
ATUIN_DB_PASSWORD_FILE="/run/secrets/atuin_db_password"
```

Trailing newlines in the file are ignored, and the rest is used as-is, so
unlike a password in `db_uri` it doesn't need escaping. `db_uri` must name the
host, and must not contain a password as well. This works with PostgreSQL and
MySQL; SQLite has no password.

## Administration

`atuin-server` supports some administration tools.

### User Management

While `open_registration` is `true`, anyone who can reach the server can
register. To keep a server private, leave it `false` and create accounts on the
server instead:

```shell
atuin-server admin user create --email alice@example.com alice
```

You can also delete users as well as change their passwords.

```shell
atuin-server admin user set-password alice
atuin-server admin user delete alice
```

## TLS

We strongly urge you enable TLS with Atuin. Without TLS, passwords are sent
plaintext, which is wildly insecure.

For TLS/HTTPS support, we recommend using a reverse proxy such as
[nginx](https://nginx.org/), [Caddy](https://caddyserver.com/), or
[Traefik](https://traefik.io/traefik) in front of the Atuin server.

## Reverse proxies and client addresses

Behind a reverse proxy, the server only sees the proxy's address, so request
logs show that instead of the real client. Proxies pass the original address
along in a header, usually `X-Forwarded-For`, but as any client can set that
header it's only honoured for connections that come from a proxy you list in
`trusted_proxies`:

```toml
trusted_proxies = ["172.18.0.0/16", "10.0.0.1"]
```

Entries are CIDR networks or bare addresses. The same list can be given via the
environment as a comma-separated string:

```sh
ATUIN_TRUSTED_PROXIES="172.18.0.0/16,10.0.0.1"
```

If your proxy reports the client in a different header, name it with
`trusted_proxy_header` (or `ATUIN_TRUSTED_PROXY_HEADER`):

```toml
trusted_proxy_header = "X-Real-IP"
```

When a request arrives from a trusted proxy, the header is read from right to
left and the first address that isn't itself a trusted proxy is logged as the
client. Leave the list empty, the default, to always log the connecting peer.
