---
title: Self Hosting
---

While we offer a public sync server, and cannot view your data (as it is encrypted), you may still wish to self host your own Atuin sync server.

The requirements to do so are pretty minimal! You need to be able to run a binary or docker container, and have a PostgreSQL database setup. Atuin requires PostgreSQL 14 or above.

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


| Parameter           | Description                                                                   |
| ------------------- | ----------------------------------------------------------------------------- |
| `host`              | The host to listen on (default: 127.0.0.1)                                    |
| `port`              | The port to listen on (default: 8888)                                         |
| `open_registration` | If `true`, accept new user registrations (default: false)                     |
| `db_uri`            | A valid PostgreSQL URI, for saving history (default: false)                   |
| `path`              | A path to prepend to all routes of the server (default: false)                |

