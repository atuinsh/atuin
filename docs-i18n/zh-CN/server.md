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

### path

path 指的是给 server 添加的路由前缀。值为空字符串将不会添加路由前缀。

默认为 `""`

## 容器部署说明

你可以在容器中部署自己的 atuin 服务器：

* 有关 docker 配置的示例，请参考 [docker](docker.md)。
* 有关 kubernetes 配置的示例，请参考 [k8s](k8s.md)。
