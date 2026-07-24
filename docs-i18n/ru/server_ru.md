# `atuin server`

Atuin позволяет запустить свой собственный сервер синхронизации, если вы 
не хотите использовать мой :)

Здесь есть только одна субкоманда, `atuin server start`, которая запустит 
Atuin http-сервер синхронизации

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

## config

Серверная конфигурация лежит отдельно от файла пользовательской, даже если
это один и тот же бинарный файл. Серверная конфигурация лежит в `~/.config/atuin/server.toml`.

Этот файл выглядит как-то так:

```toml
host = "0.0.0.0"
port = 8888
open_registration = true
db_uri="postgres://user:password@hostname/database"
```

Конфигурация так же может находиться в переменных окружения.

```sh
ATUIN_HOST="0.0.0.0"
ATUIN_PORT=8888
ATUIN_OPEN_REGISTRATION=true
ATUIN_DB_URI="postgres://user:password@hostname/database"
```

### host

Адрес хоста, который будет прослушиваться сервером Atuin 

По умолчанию это `127.0.0.1`.

### port

Порт, который будет прослушиваться сервером Atuin.

По умолчанию это `8888`.

### open_registration

Если `true`, autin будет разрешать регистрацию новых пользователей.
Установите флаг `false`, если после создания вашего аккаунта вы не хотите, чтобы другие 
могли пользоваться вашим сервером.

По умолчанию `false`.

### db_uri

Действующий URI postgres, где будет сохранён аккаунт пользователя и история.

## Docker

Поддерживается образ Docker чтобы сделать проще развертывание сервера в контейнере. Тег `latest` не публикуется, поэтому найдите номер последнего релиза на [странице релизов](https://github.com/atuinsh/atuin/releases) и подставьте его вместо `<LATEST TAGGED RELEASE>`.

```sh
docker run -d -v "$USER/.config/atuin:/config" ghcr.io/atuinsh/atuin:<LATEST TAGGED RELEASE> start
```

## Docker Compose

Использование вашего собственного docker-образа с хостингом вашего собственного Atuin может быть реализовано через 
файл docker-compose. 

Создайте файл `.env` рядом с `docker-compose.yml` с содержанием наподобие этому:

```
ATUIN_DB_USERNAME=atuin
# Choose your own secure password
ATUIN_DB_PASSWORD=really-insecure
```

Создайте `docker-compose.yml`:

```yaml
version: '3.5'
services:
  atuin:
    restart: always
    image: ghcr.io/atuinsh/atuin:<LATEST TAGGED RELEASE>
    command: start
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
    volumes: # Don't remove permanent storage for index database files!
      - "./database:/var/lib/postgresql/data/"
    environment:
      POSTGRES_USER: $ATUIN_DB_USERNAME
      POSTGRES_PASSWORD: $ATUIN_DB_PASSWORD
      POSTGRES_DB: atuin
```

Запустите службы с помощью `docker-compose`:

```sh
docker-compose up -d
```

### Использование systemd для управления сервером Atuin

`systemd` юнит чтобы управлять службами, контролируемыми `docker-compose`:

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

Включите и запустите службу командой:

```sh
systemctl enable --now atuin
```

Проверьте, работает ли:

```sh
systemctl status atuin
```

