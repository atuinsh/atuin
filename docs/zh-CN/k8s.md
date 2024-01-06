# Kubernetes

你可以使用 Kubernetes 来托管你的 Atuin 服务器。

为数据库凭证创建 [`secrets.yaml`](../../k8s/secrets.yaml) 文件：

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

为 Atuin 服务器创建 [`atuin.yaml`](../../k8s/atuin.yaml) 文件：


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

最后，你可能想让 atuin 使用单独的命名空间（namespace），创建 [`namespace.yaml`](../../k8s/namespaces.yaml) 文件：

```yaml
apiVersion: v1
kind: Namespace
metadata:
  name: atuin-namespace
  labels:
    name: atuin
```

在企业级安装部署时，你可能想要数据库内容永久存储在集群中，而不是在主机系统中。在上述配置中，`storageClassName` 配置为 `manual`，主机系统的挂载目录配置为 `/Users/firstname.lastname/.kube/database`，请注意，这些配置将会使得数据库内容存储在 kubernetes 集群<i>外部</i>中。

你还应该将 `secrets.yaml` 文件中的 `ATUIN_DB_PASSWORD` 和 `ATUIN_DB_URI` 修改为更安全的加密字符串。

Atuin 运行在主机系统的 `30530` 端口上。这是通过 `nodePort` 属性进行陪你的。Kubernetes 有一个严格规则，即不允许暴露小于 30000 的端口号。为了使客户端能够正常工作，你需要在你的 `config.toml` 文件中设置端口号，例如 `sync_address = "http://192.168.1.10:30530"`。

使用 `kubectl` 部署 Atuin 服务器：

```shell
  kubectl apply -f ./namespaces.yaml
  kubectl apply -n atuin-namespace \
                -f ./secrets.yaml \
                -f ./atuin.yaml
```

上面示例同时也位于 atuin 仓库（repository）的 [k8s](../../k8s) 目录下。
