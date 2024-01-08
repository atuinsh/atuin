# Kubernetes

You could host your own Atuin server using the Kubernetes platform.

Create a [`secrets.yaml`](../../../k8s/secrets.yaml) file for the database credentials:

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

Create a [`atuin.yaml`](../../../k8s/atuin.yaml) file for the Atuin server:

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
          image: ghcr.io/atuinsh/atuin:latest
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

Finally, you may want to use a separate namespace for atuin, by creating a [`namespaces.yaml`](../../../k8s/namespaces.yaml) file:

```yaml
apiVersion: v1
kind: Namespace
metadata:
  name: atuin-namespace
  labels:
    name: atuin
```

Note that this configuration will store the database folder _outside_ the kubernetes cluster, in the folder `/Users/firstname.lastname/.kube/database` of the host system by configuring the `storageClassName` to be `manual`. In a real enterprise setup, you would probably want to store the database content permanently in the cluster, and not in the host system.

You should also change the password string in `ATUIN_DB_PASSWORD` and `ATUIN_DB_URI` in the`secrets.yaml` file to a more secure one.

The atuin service on the port `30530` of the host system. That is configured by the `nodePort` property. Kubernetes has a strict rule that you are not allowed to expose a port numbered lower than 30000. To make the clients work, you can simply set the port in in your `config.toml` file, e.g. `sync_address = "http://192.168.1.10:30530"`.

Deploy the Atuin server using `kubectl`:

```shell
  kubectl apply -f ./namespaces.yaml
  kubectl apply -n atuin-namespace \
                -f ./secrets.yaml \
                -f ./atuin.yaml
```

The sample files above are also in the [k8s](../../../k8s/) folder of the atuin repository.
