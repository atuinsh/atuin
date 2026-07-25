# store

## `atuin store`

Atuin 会把你的历史记录保存在一个经过加密、仅追加写入的**记录存储（record store）**中。同步的原理是交换这些记录，而不是在数据库之间复制行。`atuin store` 用于检查和修复这个存储。

大多数人从来不需要用到这些命令，只有在同步出现异常，或者某台机器上残留着用它已不再持有的密钥加密的记录时，才需要用到它们。

!!! danger
    `rekey`、`purge` 以及 `push`/`pull` 的 `--force` 形式可能会造成无法恢复的数据损失。运行某个命令之前，请先阅读它的说明，并确认你清楚 Atuin 当前使用的是哪个密钥。

## 子命令

### `atuin store status`

打印记录存储的当前状态——本地存在多少条记录，并按标签和按主机分别统计。

```shell
atuin store status
```

诊断同步问题时，请从这里开始。

### `atuin store verify`

检查每一条本地记录是否都能用你当前的密钥解密。

```shell
atuin store verify
```

出现失败意味着有些记录是用另一个密钥写入的——通常是因为某台机器曾用旧密钥登录，或者密钥被重新生成过。

### `atuin store purge`

删除解密失败的本地记录。

```shell
atuin store purge
```

!!! warning
    这只会影响当前机器上的本地记录存储，不会清除你的历史记录、删除你的同步账户，也不会影响其他机器。

请先运行 `atuin store verify`，确认你清理的确实是真正的密钥不匹配问题，而不是在盲目删除数据。完整流程请参见
[删除历史记录](../guide/delete-history.md#purging-undecryptable-local-store-records)。

### `atuin store rekey [KEY]`

用新密钥重新加密整个本地存储。省略密钥参数将自动为你生成一个。

```shell
atuin store rekey
```

!!! danger
    此操作之后，其他每台机器都需要拿到这个新密钥，才能读取你同步的任何内容。请先把新密钥保存到安全的地方。

### `atuin store rebuild <TAG>`

根据记录存储重建派生状态——例如，根据 `history` 记录重新生成历史数据库。

```shell
atuin store rebuild history
```

当记录存储本身完好、但其本地视图出了问题时，这个命令很有用。

### `atuin store push`

单向上传：将本地记录上传到同步服务器。

| 标志 | 说明 |
|------|-------------|
| `--tag`/`-t` | 只推送该标签（例如 `history`）。默认推送所有标签 |
| `--host` | 只推送该主机，以主机 UUID 的形式给出。默认是当前主机 |
| `--force` | 清空远程存储，然后上传本地的全部内容，涵盖所有主机和标签 |
| `--page` | 每次上传的记录数量（默认：100） |

### `atuin store pull`

单向下载：从同步服务器下载记录。

| 标志 | 说明 |
|------|-------------|
| `--tag`/`-t` | 只拉取该标签。默认拉取所有标签 |
| `--force` | 先清空本地存储，再从远程下载全部内容 |
| `--page` | 每次下载的记录数量（默认：100） |

如需常规的双向同步，请改用 [`atuin sync`](sync.md)。
