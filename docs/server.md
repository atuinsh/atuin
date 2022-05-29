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

The port the atuin server should listen on.

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

## Docker Compose

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

Start the services using `docker-compose`:

```sh
docker-compose up -d
```

### Using systemd to manage your atuin server

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

## Kubernetes

You could host your own Atuin server using the Kubernetes platform.

Create a `secrets.yaml` file for the database credentials:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: atuin-secrets
type: Opaque
stringData:
  ATUIN_DB_USERNAME: atuin
  ATUIN_DB_PASSWORD: seriously-insecure
  ATUIN_HOST: "127.0.0.1"
  ATUIN_PORT: "8888"
  ATUIN_OPEN_REGISTRATION: "true"
  ATUIN_DB_URI: "postgres://atuin:seriously-insecure@localhost/atuin"
immutable: true
```

Create a `atuin.yaml` file for the Atuin server:

```yaml
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: atuin
spec:
  replicas: 1
  selector:
    matchLabels:
      io.kompose.service: atuin
  template:
    metadata:
      labels:
        io.kompose.service: atuin
    spec:
      containers:
        - args:
            - server
            - start
          env:
            - name: ATUIN_DB_URI
              valueFrom:
                secretKeyRef:
                  name: atuin-secrets
                  key: ATUIN_DB_URI
                  optional: false
            - name: ATUIN_HOST
              value: 0.0.0.0
            - name: ATUIN_PORT
              value: "8888"
            - name: ATUIN_OPEN_REGISTRATION
              value: "true"
          image: ghcr.io/ellie/atuin:main
          name: atuin
          ports:
            - containerPort: 8888
          resources:
            limits:
              cpu: 250m
              memory: 1Gi
            requests:
              cpu: 250m
              memory: 1Gi
          volumeMounts:
            - mountPath: /config
              name: atuin-claim0
        - name: postgresql
          image: postgres:14
          ports:
            - containerPort: 5432
          env:
            - name: POSTGRES_DB
              value: atuin
            - name: POSTGRES_PASSWORD
              valueFrom:
                secretKeyRef:
                  name: atuin-secrets
                  key: ATUIN_DB_PASSWORD
                  optional: false
            - name: POSTGRES_USER
              valueFrom:
                secretKeyRef:
                  name: atuin-secrets
                  key: ATUIN_DB_USERNAME
                  optional: false
          resources:
            limits:
              cpu: 250m
              memory: 1Gi
            requests:
              cpu: 250m
              memory: 1Gi
          volumeMounts:
            - mountPath: /var/lib/postgresql/data/
              name: database
      volumes:
        - name: database
          persistentVolumeClaim:
            claimName: database
        - name: atuin-claim0
          persistentVolumeClaim:
            claimName: atuin-claim0
---
apiVersion: v1
kind: Service
metadata:
  labels:
    io.kompose.service: atuin
  name: atuin
spec:
  type: NodePort
  ports:
    - name: "8888"
      port: 8888
      nodePort: 30530
  selector:
    io.kompose.service: atuin
---
kind: PersistentVolume
apiVersion: v1
metadata:
  name: database-pv
  labels:
    app: database
    type: local
spec:
  storageClassName: manual
  capacity:
    storage: 300Mi
  accessModes:
    - ReadWriteOnce
  hostPath:
    path: "/Users/firstname.lastname/.kube/database"
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  labels:
    io.kompose.service: database
  name: database
spec:
  storageClassName: manual
  accessModes:
    - ReadWriteOnce
  resources:
    requests:
      storage: 300Mi
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  labels:
    io.kompose.service: atuin-claim0
  name: atuin-claim0
spec:
  accessModes:
    - ReadWriteOnce
  resources:
    requests:
      storage: 10Mi
```

Note that this configuration will store the database folder _outside_ the kubernetes cluster, in the folder `/Users/firstname.lastname/.kube/database` of the host system by configuring the `storageClassName` to be `manual`. In a real enterprise setup, you would probably want to store the database content permanently in the cluster, and not in the host system.

You should also change the password string in `ATUIN_DB_PASSWORD` and `ATUIN_DB_URI` in the`secrets.yaml` file to a more secure one.

The atuin service on the port `30530` of the host system. That is configured by the `nodePort` property. Kubernetes has a strict rule that you are not allowed to expose a port numbered lower than 30000. To make the clients work, you can simply set the port in in your `config.toml` file, e.g. `sync_address = "http://192.168.1.10:30530"`.

Deploy the Atuin server using `kubectl`:

```shell
kubectl apply -f secrets.yaml
kubectl apply -f atuin.yaml
```
