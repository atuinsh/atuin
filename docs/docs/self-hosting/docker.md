# Docker

!!! warning
    If you are self hosting, we strongly suggest you stick to [tagged releases](https://github.com/atuinsh/atuin/releases), and do not follow `main` or `latest`

    Follow the GitHub releases, and please read the notes for each release. Most of the time, upgrades can occur without any manual intervention.

    We cannot guarantee that all updates will apply cleanly, and some may require some extra steps.

There is a supplied docker image to make deploying a server as a container easier. The "LATEST TAGGED RELEASE" can be found on the [releases page](https://github.com/atuinsh/atuin/releases).

```sh
CONFIG="$HOME/.config/atuin"
mkdir "$CONFIG"
chown 1000:1000 "$CONFIG"
docker run -d -v "$CONFIG:/config" ghcr.io/atuinsh/atuin:<LATEST TAGGED RELEASE> start
```

## Docker Compose

Using the already build docker image hosting your own Atuin can be done using the supplied docker-compose file.

Create a `.env` file next to [`docker-compose.yml`](https://github.com/atuinsh/atuin/raw/main/docker-compose.yml) with contents like this:

```ini
ATUIN_DB_NAME=atuin
ATUIN_DB_USERNAME=atuin
# Choose your own secure password. Stick to [A-Za-z0-9.~_-]
ATUIN_DB_PASSWORD=really-insecure
```

Create a `docker-compose.yml`:

```yaml
services:
  atuin:
    restart: always
    image: ghcr.io/atuinsh/atuin:<LATEST TAGGED RELEASE>
    command: start
    volumes:
      - "./config:/config"
    ports:
      - 8888:8888
    environment:
      ATUIN_HOST: "0.0.0.0"
      ATUIN_OPEN_REGISTRATION: "true"
      ATUIN_DB_URI: postgres://${ATUIN_DB_USERNAME}:${ATUIN_DB_PASSWORD}@db/${ATUIN_DB_NAME}
      RUST_LOG: info,atuin_server=debug
    depends_on:
      - db
  db:
    image: postgres:18
    restart: unless-stopped
    volumes: # Don't remove permanent storage for index database files!
      - "./database:/var/lib/postgresql/"
    environment:
      POSTGRES_USER: ${ATUIN_DB_USERNAME}
      POSTGRES_PASSWORD: ${ATUIN_DB_PASSWORD}
      POSTGRES_DB: ${ATUIN_DB_NAME}
```

Start the services using `docker compose`:

```sh
mkdir config
chown 1000:1000 config
docker compose up -d
```

## Using systemd to manage your atuin server

The following `systemd` unit file to manage your `docker-compose` managed service:

```ini
[Unit]
Description=Docker Compose Atuin Service
Requires=docker.service
After=docker.service

[Service]
# Where the docker-compose file is located
WorkingDirectory=/srv/atuin-server
ExecStart=/usr/bin/docker compose up
ExecStop=/usr/bin/docker compose down
TimeoutStartSec=0
Restart=on-failure
StartLimitBurst=3

[Install]
WantedBy=multi-user.target
```

Start and enable the service with:

```sh
systemctl enable --now atuin
```

Check if its running with:

```sh
systemctl status atuin
```

## Creating backups of the Postgres database

You can add another service to your `docker-compose.yml` file to have it run daily backups. It should look like this:

```yaml
  backup:
    container_name: atuin_db_dumper
    image: prodrigestivill/postgres-backup-local
    env_file:
      - .env
    environment:
      POSTGRES_HOST: postgresql
      POSTGRES_DB: ${ATUIN_DB_NAME}
      POSTGRES_USER: ${ATUIN_DB_USERNAME}
      POSTGRES_PASSWORD: ${ATUIN_DB_PASSWORD}
      SCHEDULE: "@daily"
      BACKUP_DIR: /db_dumps
    volumes:
      - ./db_dumps:/db_dumps
    depends_on:
      - postgresql
```

This will create daily backups of your database for that additional layer of comfort.

PLEASE NOTE: The `./db_dumps` mount MUST be a POSIX-compliant filesystem to store the backups (mainly with support for hardlinks and softlinks). So filesystems like VFAT, EXFAT, SMB/CIFS, ... can't be used with this docker image. See https://github.com/prodrigestivill/docker-postgres-backup-local for more details on how this works. There are additional settings for the number of backups retained, etc., all explained on the linked page.
