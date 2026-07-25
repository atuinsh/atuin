# 服务器设置

我们提供了公共同步服务器，而且由于数据是加密的，我们也无法看到你的数据；即便如此，你可能仍然希望自托管一台 Atuin 同步服务器。

这样做的门槛其实很低：你只需要能够运行一个二进制文件或 docker 容器，并准备好一个 PostgreSQL 数据库。Atuin 要求 PostgreSQL 14 或更高版本。

此外，服务器也可以使用 SQLite（3 或更高版本）来代替 PostgreSQL。

服务器以独立的 `atuin-server` 二进制文件形式发布。每个版本都会在 [GitHub releases 页面](https://github.com/atuinsh/atuin/releases)上提供预编译的二进制文件和安装脚本。例如，要安装最新版本：

```shell
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/atuinsh/atuin/releases/latest/download/atuin-server-installer.sh | sh
```

安装完成后，使用以下命令启动服务器：

```shell
atuin-server start
```

!!! note
    在 v18.12.0 之前，服务器是打包在主 `atuin` 二进制文件里的，通过 `atuin server start` 启动。如果你正在从旧版本升级，需要安装新的 `atuin-server` 二进制文件，并更新所有服务文件（systemd、docker、k8s），让它们调用 `atuin-server` 而不是 `atuin server`。详情请参阅[发行说明](https://github.com/atuinsh/atuin/releases)。

## 配置

服务器的配置文件位于 `~/.config/atuin/server.toml`，与客户端配置文件是分开的。

对于 PostgreSQL，配置大致如下：

```toml
host = "0.0.0.0"
port = 8888
open_registration = true
db_uri="postgres://user:password@hostname/database"
```

配置同样可以通过环境变量提供。

```sh
ATUIN_HOST="0.0.0.0"
ATUIN_PORT=8888
ATUIN_OPEN_REGISTRATION=true
ATUIN_DB_URI="postgres://user:password@hostname/database"
```

| 参数                | 描述                                                    |
| ------------------- | -------------------------------------------------------------- |
| `host`              | 监听的主机地址（默认：127.0.0.1）                     |
| `port`              | 监听的 TCP 端口（默认：8888）                      |
| `open_registration` | 若为 `true`，则接受新用户注册（默认：false）      |
| `db_uri`            | 用于保存历史记录的有效 PostgreSQL 或 SQLite URI（必填，无默认值） |
| `path`              | 添加在服务器所有路由之前的路径前缀（默认：空） |

对于 SQLite，请在 server.toml 中使用以下配置：

```toml
db_uri="sqlite:///config/atuin.db"
```

另外，也可以通过环境变量提供数据库 URI。

```sh
ATUIN_DB_URI="sqlite:///config/atuin.db"
```

这样会在 `/config` 目录下创建数据库。请务必将一个持久化卷挂载到 `/config` 目录，并确保该目录对 Atuin 服务器可写。

### TLS

若需要 TLS/HTTPS 支持，建议在 Atuin 服务器前面部署反向代理，例如 nginx、Caddy 或 Traefik。这是容器化应用的标准做法，能为证书管理带来更好的灵活性。

> **注意：** 内置的 `[tls]` 配置选项已被移除。如果你之前使用过该选项，请迁移到反向代理方案。配置文件中任何现有的 `[tls]` 部分都将被忽略。
