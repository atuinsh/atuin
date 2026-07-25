# Kubernetes

!!! warning
    如果你在自托管，我们强烈建议你只使用带标签的发布版本，不要跟随 `main` 或 `latest`。

    请关注 GitHub 上的发布版本，并阅读每个版本的发布说明。大多数情况下，升级都可以在无需任何人工干预的情况下完成。

    我们无法保证所有更新都能顺利应用，有些更新可能需要额外的步骤。

你可以使用 Kubernetes 平台托管自己的 Atuin 服务器。

创建一个 [`secrets.yaml`](https://github.com/atuinsh/atuin/blob/main/k8s/secrets.yaml) 文件，用于存放数据库凭据：

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
  ATUIN_DB_URI: "postgres://atuin:seriously-insecure@postgres/atuin"
immutable: true
```

创建一个 [`atuin.yaml`](https://github.com/atuinsh/atuin/blob/main/k8s/atuin.yaml) 文件用于 Atuin 服务器。和 Docker 镜像一样，这里也没有 `latest` 标签，所以请将「LATEST TAGGED RELEASE」替换为[发布页面](https://github.com/atuinsh/atuin/releases)中最新的带标签发布版本：

```yaml
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: postgres
  namespace: atuin
spec:
  replicas: 1
  strategy:
    type: Recreate # This is important to ensure duplicate pods don't run and cause corruption
  selector:
    matchLabels:
      io.kompose.service: postgres
  template:
    metadata:
      labels:
        io.kompose.service: postgres
    spec:
      containers:
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
          lifecycle:
            preStop:
              exec:
                # This ensures graceful shutdown see: https://stackoverflow.com/a/75829325/3437018
                # Potentially consider using a `StatefulSet` instead of a `Deployment`
                command: ["/usr/local/bin/pg_ctl stop -D /var/lib/postgresql/data -w -t 60 -m fast"]
          resources:
            requests:
              cpu: 100m
              memory: 100Mi
            limits:
              cpu: 250m
              memory: 600Mi
          volumeMounts:
            - mountPath: /var/lib/postgresql/data/
              name: database
      volumes:
        - name: database
          persistentVolumeClaim:
            claimName: database
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
          image: ghcr.io/atuinsh/atuin:<LATEST TAGGED RELEASE>
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
      volumes:
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
apiVersion: v1
kind: Service
metadata:
  labels:
    io.kompose.service: postgres
  name: postgres
spec:
  type: ClusterIP
  selector:
    io.kompose.service: postgres
  ports:
    - protocol: TCP
      port: 5432
      targetPort: 5432
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

最后，你可能希望为 Atuin 使用一个单独的命名空间，为此可以创建一个 [`namespaces.yaml`](https://github.com/atuinsh/atuin/blob/main/k8s/namespaces.yaml) 文件：

```yaml
apiVersion: v1
kind: Namespace
metadata:
  name: atuin-namespace
  labels:
    name: atuin
```

请注意，此配置会将数据库文件夹存储在 Kubernetes 集群 _外部_，即通过将 `storageClassName` 配置为 `manual`，存放在宿主系统的 `/Users/firstname.lastname/.kube/database` 文件夹中。在真正的企业环境中，你可能希望将数据库内容永久存储在集群内，而不是宿主系统上。

你还应该将 `secrets.yaml` 文件中 `ATUIN_DB_PASSWORD` 和 `ATUIN_DB_URI` 里的密码字符串修改为更安全的值。

Atuin 服务通过宿主系统的 `30530` 端口对外暴露，这由 `nodePort` 属性配置。Kubernetes 有一条严格的规则，不允许暴露编号低于 30000 的端口。为了让客户端正常工作，请在你的 `config.toml` 文件中设置该端口，例如 `sync_address = "http://192.168.1.10:30530"`。

使用 `kubectl` 部署 Atuin 服务器：

```shell
  kubectl apply -f ./namespaces.yaml
  kubectl apply -n atuin-namespace \
                -f ./secrets.yaml \
                -f ./atuin.yaml
```

上述示例文件也可以在 `atuin` 仓库的 [k8s 文件夹](https://github.com/atuinsh/atuin/tree/main/k8s)中找到。

## 创建 Postgres 数据库的备份

现在服务已经启动并运行，是时候考虑备份问题了。

你可以创建一个 [`CronJob`](https://kubernetes.io/docs/concepts/workloads/controllers/cron-jobs/)，使用 [`pg_dump`](https://www.postgresql.org/docs/current/app-pgdump.html) 来创建数据库备份。这个示例每周运行一次，并将备份转储到节点的本地磁盘上。

```yaml
apiVersion: batch/v1
kind: CronJob
metadata:
  name: atuin-db-backup
spec:
  schedule: "0 0 * * 0" # 每周日午夜运行
  jobTemplate:
    spec:
      template:
        spec:
          containers:
          - name: atuin-db-backup-pg-dump
            image: postgres:14
            command: [
              "/bin/bash",
              "-c",
              "pg_dump --host=postgres --username=atuin --format=c --file=/backup/atuin-backup-$(date +'%Y-%m-%d').pg_dump",
            ]
            env:
              - name: PGPASSWORD
                valueFrom:
                  secretKeyRef:
                    name: atuin-secrets
                    key: ATUIN_DB_PASSWORD
                    optional: false
            volumeMounts:
            - name: backup-volume
              mountPath: /backup
          restartPolicy: OnFailure
          volumes:
          - name: backup-volume
            hostPath:
              path: /somewhere/on/node/for/database-backups
              type: Directory
```

请根据以下说明配置或更新示例 `yaml`：
- 通过 `schedule` 属性设置更频繁或更不频繁的计划。
- 将 `/somewhere/on/node/for/database-backups` 替换为你节点上的某个路径，或者改用 `PersistentVolume` 而不是 `hostPath`。
- `--format=c` 输出的格式可以用 `pg_restore` 还原。如果你想改为输出 `.sql` 文件，请使用 [`plain`](https://www.postgresql.org/docs/current/app-pgdump.html)。
