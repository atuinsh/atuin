# `atuin server`

Atuin vam omogućava da pokrenete sopstveni server za sinhronizaciju, ako
ne želite da koristite podrazumevani :)

Ovde postoji samo jedna podkomanda, `atuin server start`, koja pokreće
Atuin http server za sinhronizaciju

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

Serverska konfiguracija se čuva odvojeno od korisničkog fajla konfiguracije, čak i ako
je u pitanju isti binarni fajl. Serverska konfiguracija se nalazi u `~/.config/atuin/server.toml`.

Ovaj fajl izgleda otprilike ovako:

```toml
host = "0.0.0.0"
port = 8888
open_registration = true
db_uri="postgres://user:password@hostname/database"
```

Konfiguracija takođe može biti smeštena u promenljive okruženja.

```sh
ATUIN_HOST="0.0.0.0"
ATUIN_PORT=8888
ATUIN_OPEN_REGISTRATION=true
ATUIN_DB_URI="postgres://user:password@hostname/database"
```

### host

Adresa hosta na kojoj će Atuin server osluškivati.

Podrazumevano je `127.0.0.1`.

### port

Port na kojem će Atuin server osluškivati.

Podrazumevano je `8888`.

### open_registration

Ako je `true`, Atuin će dozvoliti registraciju novih korisnika.
Postavite na `false` ako nakon kreiranja vašeg naloga ne želite da drugi
koriste vaš server.

Podrazumevano je `false`.

### db_uri

Validan postgres URI gde će biti sačuvan nalog korisnika i istorija.

## Docker

Podržan je Docker image kako bi se olakšalo pokretanje servera u kontejneru.

```sh
docker run -d -v "$USER/.config/atuin:/config" ghcr.io/ellie/atuin:latest server start
```

## Docker Compose

Hostovanje sopstvenog Atuin servera pomoću vašeg docker-image-a može biti realizovano kroz
docker-compose fajl.

Kreirajte `.env` fajl pored `docker-compose.yml` sa sadržajem nalik ovom:

```
ATUIN_DB_USERNAME=atuin
# Choose your own secure password
ATUIN_DB_PASSWORD=really-insecure
```

Kreirajte `docker-compose.yml`:

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
    volumes: # Don't remove permanent storage for index database files!
      - "./database:/var/lib/postgresql/data/"
    environment:
      POSTGRES_USER: $ATUIN_DB_USERNAME
      POSTGRES_PASSWORD: $ATUIN_DB_PASSWORD
      POSTGRES_DB: atuin
```

Pokrenite servise pomoću `docker-compose`:

```sh
docker-compose up -d
```

### Korišćenje systemd za upravljanje Atuin serverom

`systemd` unit za upravljanje servisima koje kontroliše `docker-compose`:

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

Omogućite i pokrenite servis komandom:

```sh
systemctl enable --now atuin
```

Proverite da li radi:

```sh
systemctl status atuin
```
