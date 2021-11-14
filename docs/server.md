# `atuin server`

Atuin allows you to run your own sync server, in case you don't want to use the
one I host :)

There's currently only one subcommand, `atuin server start` which will start the
Atuin http sync server

```
USAGE:
    atuin server start [OPTIONS]

FLAGS:
        --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -h, --host <host>
    -p, --port <port>
```

## Configuration

The config for the server is kept separate from the config for the client, even
though they are the same binary. Server config can be found at
`~/.config/atuin/server.toml`.

It looks something like this:

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

### host

The host address the atuin server should listen on.

Defaults to `127.0.0.1`.

### port

The post the atuin server should listen on.

Defaults to `8888`.

### open_registration

If `true`, atuin will accept new user registrations.
Set this to `false` after making your own account if you don't want others to be
able to use your server.

Defaults to `false`.

### db_uri

A valid postgres URI, where the user and history data will be saved to.

## Docker

There is a supplied docker image to make deploying a server as a container easier.

```sh
docker run -d -v "$USER/.config/atuin:/config" ghcr.io/ellie/atuin:latest server start
```