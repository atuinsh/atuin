# Kubernetes

Você pode usar o Kubernetes para hospedar seu servidor Atuin.

Crie o arquivo `secrets.yaml` para as credenciais do banco de dados:

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

Crie o arquivo `atuin.yaml` para o servidor Atuin:

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

Finalmente, você pode querer que o Atuin use um namespace separado, crie o arquivo `namespace.yaml`:

```yaml
apiVersion: v1
kind: Namespace
metadata:
  name: atuin-namespace
  labels:
    name: atuin
```

Ao implantar em um ambiente de produção, você provavelmente desejará que o conteúdo do banco de dados seja armazenado persistentemente no cluster, em vez de no sistema host. Na configuração acima, `storageClassName` está definido como `manual` e o diretório de montagem do sistema host está definido como `/Users/firstname.lastname/.kube/database`. Observe que essas configurações farão com que o conteúdo do banco de dados seja armazenado *fora* do cluster Kubernetes.

Você também deve alterar `ATUIN_DB_PASSWORD` e `ATUIN_DB_URI` no arquivo `secrets.yaml` para strings criptografadas mais seguras.

O Atuin é executado na porta `30530` do sistema host. Isso é configurado através da propriedade `nodePort`. O Kubernetes tem uma regra estrita que não permite expor números de porta menores que 30000. Para que o cliente funcione corretamente, você precisará definir o número da porta no seu arquivo `config.toml`, por exemplo, `sync_address = "http://192.168.1.10:30530"`.

Use `kubectl` para implantar o servidor Atuin:

```shell
  kubectl apply -f ./namespaces.yaml
  kubectl apply -n atuin-namespace \
                -f ./secrets.yaml \
                -f ./atuin.yaml
```

O exemplo acima também está localizado no diretório [k8s](../../k8s) do repositório do Atuin.