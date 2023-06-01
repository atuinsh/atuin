# Docker

There is a supplied docker image to make deploying a server as a container easier.

```sh
docker run -d -v "$HOME/.config/atuin:/config" ghcr.io/atuinsh/atuin:latest server start
```

# Docker Compose

Using the already build docker image hosting your own Atuin can be done using the supplied docker-compose file.

Create a `.env` file next to `docker-compose.yml` with contents like this:

```
ATUIN_DB_USERNAME=atuin
# Choose your own secure password
ATUIN_DB_PASSWORD=really-insecure
```

Create a `docker-compose.yml`:

```yaml
version: '3.5'
services:
  atuin:
    image: ghcr.io/atuinsh/atuin:main
    restart: always
    user: ${UID:-1000}:${GID:-1000}
    command: server start
    volumes:
      - "./config:/config"
    links:
      - postgresql:db
    ports:
      - 8888:8888
    environment:
      ATUIN_HOST: "0.0.0.0"
      ATUIN_OPEN_REGISTRATION: "true"
      ATUIN_DB_URI: postgres://$ATUIN_DB_USERNAME:$ATUIN_DB_PASSWORD@db/atuin
  postgresql:
    image: postgres:14-alpine
    restart: always
    user: ${UID:-1000}:${GID:-1000}
    volumes: # Don't remove permanent storage for index database files!
      - "./database:/var/lib/postgresql/data/"
    environment:
      POSTGRES_USER: $ATUIN_DB_USERNAME
      POSTGRES_PASSWORD: $ATUIN_DB_PASSWORD
      POSTGRES_DB: atuin
```

Start the services using `docker-compose`:

```sh
docker-compose up -d
```

## Using systemd to manage your atuin server

The following `systemd` unit file to manage your `docker-compose` managed service:

```
[Unit]
Description=Docker Compose Atuin Service
Requires=docker.service
After=docker.service

[Service]
# Where the docker-compose file is located
WorkingDirectory=/srv/atuin-server
ExecStart=/usr/bin/docker-compose up
ExecStop=/usr/bin/docker-compose down
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
