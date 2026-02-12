# Docker

O Atuin fornece uma imagem Docker que facilita a implantação do servidor como um contêiner.

```sh
docker run -d -v "$USER/.config/atuin:/config" ghcr.io/ellie/atuin:latest server start
```

# Docker Compose

Para hospedar seu próprio Atuin usando imagens Docker existentes, você pode usar o arquivo `docker-compose` fornecido.

Crie um arquivo `.env` no mesmo diretório do `docker-compose.yml` com o seguinte conteúdo:

```
ATUIN_DB_USERNAME=atuin
# Preencha sua senha
ATUIN_DB_PASSWORD=really-insecure
```

Crie o arquivo `docker-compose.yml`:

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
    volumes: # Não exclua o armazenamento persistente do arquivo de banco de dados de índice!
      - "./database:/var/lib/postgresql/data/"
    environment:
      POSTGRES_USER: $ATUIN_DB_USERNAME
      POSTGRES_PASSWORD: $ATUIN_DB_PASSWORD
      POSTGRES_DB: atuin
```

Use `docker-compose` para iniciar o serviço:

```sh
docker-compose up -d
```

## Gerenciando seu servidor Atuin com systemd

O seguinte arquivo de configuração do `systemd` é usado para gerenciar seu serviço hospedado pelo `docker-compose`:

```
[Unit]
Description=Docker Compose Atuin Service
Requires=docker.service
After=docker.service

[Service]
# Onde o arquivo docker-compose está localizado
WorkingDirectory=/srv/atuin-server
ExecStart=/usr/bin/docker-compose up
ExecStop=/usr/bin/docker-compose down
TimeoutStartSec=0
Restart=on-failure
StartLimitBurst=3

[Install]
WantedBy=multi-user.target
```

Habilite o serviço:

```sh
systemctl enable --now atuin
```

Verifique se o serviço está funcionando corretamente:

```sh
systemctl status atuin
```
