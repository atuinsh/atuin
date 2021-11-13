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

The config for the server is kept separate from the config for the client, even
though they are the same binary. Server config can be found at
`~/.config/atuin/server.toml`.

It looks something like this:

```
host = "0.0.0.0"
port = 8888
open_registration = true
db_uri="postgres://user:password@hostname/database"
```

`open_registration` sets whether the server allows new user registrations. Set
this to false after making your own account if you don't want others to be able
to use your server.

## Docker

There is a supplied docker image to make deploying a server as a container easier.

```sh
docker run -d -v "$USER/.config/atuin:/config" ghcr.io/ellie/atuin:latest server start
```
