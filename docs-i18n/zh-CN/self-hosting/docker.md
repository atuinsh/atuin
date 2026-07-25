# Docker

!!! warning
    如果你是自行托管，我们强烈建议只使用[带标签的发行版](https://github.com/atuinsh/atuin/releases)，不要跟随 `main` 或 `latest`。

    请关注 GitHub 上的发行版，并阅读每个版本的发行说明。大多数情况下，升级无需任何人工干预即可完成。

    我们无法保证所有更新都能顺利应用，某些更新可能需要额外的操作步骤。

我们提供的 docker 镜像可以让你将服务器部署为一个容器。"LATEST TAGGED RELEASE"（最新的带标签发行版）可以在[发行版页面](https://github.com/atuinsh/atuin/releases)中找到。

```sh
CONFIG="$HOME/.config/atuin"
mkdir "$CONFIG"
chown 1000:1000 "$CONFIG"
docker run -d -v "$CONFIG:/config" ghcr.io/atuinsh/atuin:<LATEST TAGGED RELEASE> start
```

## Docker Compose

你也可以使用我们提供的 docker-compose 文件，通过预构建的 docker 镜像自行托管 Atuin 服务器。

创建一个 `docker-compose.yml` 文件：

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
    volumes: # 不要移除持久化存储的索引数据库文件！
      - "./database:/var/lib/postgresql/"
    environment:
      POSTGRES_USER: ${ATUIN_DB_USERNAME}
      POSTGRES_PASSWORD: ${ATUIN_DB_PASSWORD}
      POSTGRES_DB: ${ATUIN_DB_NAME}
      TZ: Europe/London
      PGTZ: Europe/London
```

在 `docker-compose.yml` 旁边创建一个 `.env` 文件，内容类似这样：

```ini
ATUIN_DB_NAME=atuin
ATUIN_DB_USERNAME=atuin
# 选择一个你自己的安全密码，只使用 [A-Za-z0-9.~_-] 范围内的字符
ATUIN_DB_PASSWORD=really-insecure
```


使用 `docker compose` 启动服务：

```sh
mkdir config
chown 1000:1000 config
docker compose up -d
```

## 使用 systemd 管理你的 Atuin 服务器

下面的 `systemd` unit 文件可用于管理通过 `docker-compose` 运行的服务：

```ini
[Unit]
Description=Docker Compose Atuin Service
Requires=docker.service
After=docker.service

[Service]
# docker-compose 文件所在的位置
WorkingDirectory=/srv/atuin-server
ExecStart=/usr/bin/docker compose up
ExecStop=/usr/bin/docker compose down
TimeoutStartSec=0
Restart=on-failure
StartLimitBurst=3

[Install]
WantedBy=multi-user.target
```

使用以下命令启动并启用该服务：

```sh
systemctl enable --now atuin
```

使用以下命令检查它是否正在运行：

```sh
systemctl status atuin
```

## 创建 Postgres 数据库的备份

你可以在 `docker-compose.yml` 文件中添加另一个服务，让它每天自动执行备份，效果类似这样：

```yaml
  backup:
    restart: unless-stopped
    container_name: atuin_db_dumper
    image: prodrigestivill/postgres-backup-local
    env_file:
      - .env
    environment:
      POSTGRES_HOST: db
      POSTGRES_DB: ${ATUIN_DB_NAME}
      POSTGRES_USER: ${ATUIN_DB_USERNAME}
      POSTGRES_PASSWORD: ${ATUIN_DB_PASSWORD}
      SCHEDULE: "@daily"
      BACKUP_DIR: /db_dumps
      TZ: Europe/London
    volumes:
      - ./db_dumps:/db_dumps
    depends_on:
      - db
```

这样就能为你的数据库创建每日备份，为你多提供一层保障。

!!! warning

    `./db_dumps` 挂载点必须使用支持硬链接和符号链接的 POSIX 兼容文件系统。VFAT、exFAT
    和 SMB/CIFS 等文件系统无法配合此镜像使用。有关保留策略设置以及备份工作原理，请参见
    [`docker-postgres-backup-local`](https://github.com/prodrigestivill/docker-postgres-backup-local)。
