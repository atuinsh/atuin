# `atuin server`

Atuin 允许您运行自己的同步服务器，以防您不想使用我(ellie)托管的服务器 :)

目前只有一个子命令，`atuin server start`，它将启动 Atuin http 同步服务器。

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

## 配置

服务器的配置与客户端的配置是分开的，即使它们是相同的二进制文件。服务器配置可以在 `~/.config/atuin/server.toml` 找到。

它看起来像这样:

```toml
host = "0.0.0.0"
port = 8888
open_registration = true
db_uri="postgres://user:password@hostname/database"
```

另外，配置也可以用环境变量来提供。

```sh
ATUIN_HOST="0.0.0.0"
ATUIN_PORT=8888
ATUIN_OPEN_REGISTRATION=true
ATUIN_DB_URI="postgres://user:password@hostname/database"
```

### host

Atuin 服务器应该监听的地址

默认为 `127.0.0.1`.

### port

Atuin 服务器应该监听的端口

默认为 `8888`.

### open_registration

如果为 `true` ，atuin 将接受新用户注册。如果您不希望其他人能够使用您的服务器，请在创建自己的账号后将此设置为 `false` 

默认为 `false`.

### db_uri

一个有效的 postgres URI, 用户和历史记录数据将被保存到其中。

## Docker

提供了一个 docker 镜像（image），可以更轻松地将服务器部署为容器（container）。

```sh
docker run -d -v "$USER/.config/atuin:/config" ghcr.io/ellie/atuin:latest server start
```

## Docker Compose

使用已有的 docker 镜像（image）来托管你自己的 Atuin，可以使用提供的 docker-compose 文件来完成

在 `docker-compose.yml` 同级目录下创建一个 `.env` 文件，内容如下:

```
ATUIN_DB_USERNAME=atuin
# Choose your own secure password
ATUIN_DB_PASSWORD=really-insecure
```

创建一个 `docker-compose.yml` 文件:

```yaml
version: '3.5'
services:
  atuin:
    restart: always
    image: ghcr.io/ellie/atuin:main
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
    image: postgres:14
    restart: unless-stopped
    volumes: # 不要删除索引数据库文件的永久存储空间!
      - "./database:/var/lib/postgresql/data/"
    environment:
      POSTGRES_USER: $ATUIN_DB_USERNAME
      POSTGRES_PASSWORD: $ATUIN_DB_PASSWORD
      POSTGRES_DB: atuin
```

使用 `docker-compose` 启动服务:

```sh
docker-compose up -d
```

### 使用 systemd 来管理你的 Atuin 服务器

以下 `systemd` 单元文件用于管理您的 `docker-compose` 托管服务：

```
[Unit]
Description=Docker Compose Atuin Service
Requires=docker.service
After=docker.service

[Service]
# docker-compose 文件所在的位置
WorkingDirectory=/srv/atuin-server 
ExecStart=/usr/bin/docker-compose up
ExecStop=/usr/bin/docker-compose down
TimeoutStartSec=0
Restart=on-failure
StartLimitBurst=3

[Install]
WantedBy=multi-user.target
```

使用以下命令启动并启用服务：

```sh
systemctl enable --now atuin
```

检查它是否运行：

```sh
systemctl status atuin
```

