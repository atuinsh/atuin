# account

## `atuin account`

管理你的同步账户。本页是命令参考。若要了解注册和登录的完整操作步骤，请参阅
[设置同步](../guide/sync.md)。

`atuin register`、`atuin login` 和 `atuin logout` 分别是 `atuin account`
对应子命令的简写形式。

## 子命令

### `atuin account register`

在已配置的同步服务器上创建账户。

```shell
atuin account register -u <USERNAME> -e <EMAIL> -p <PASSWORD>
```

| 标志 | 说明 |
|------|-------------|
| `--username`/`-u` | 你想要的用户名。必须唯一，仅限字母数字和连字符 |
| `--email`/`-e` | 仅用于重要通知，例如安全问题 |
| `--password`/`-p` | 省略此项后 Atuin 会另行提示输入，避免密码残留在你的 shell 历史记录中 |

注册后会自动完成登录，并生成你的加密密钥。请妥善保存该密钥——参见
[`atuin key`](sync.md#key)。

### `atuin account login`

在另一台机器上登录。

```shell
atuin account login -u <USERNAME>
```

| 标志 | 说明 |
|------|-------------|
| `--username`/`-u` | 你的用户名 |
| `--password`/`-p` | 省略后 Atuin 会提示输入 |
| `--key`/`-k` | 你的加密密钥。省略后 Atuin 会提示输入 |
| `--totp-code`/`-t` | 你的双因素认证代码（如果账户已启用 2FA） |

建议省略 `--password` 和 `--key`：Atuin 会分别提示输入这两项，从而避免它们
出现在你的 shell 历史记录中。

### `atuin account logout`

```shell
atuin account logout
```

结束本地会话。你的历史记录和加密密钥仍会保留在这台机器上。

### `atuin account change-password`

```shell
atuin account change-password
```

| 标志 | 说明 |
|------|-------------|
| `--current-password`/`-c` | 省略后 Atuin 会提示输入 |
| `--new-password`/`-n` | 省略后 Atuin 会提示输入 |
| `--totp-code`/`-t` | 你的双因素认证代码（如果账户已启用 2FA） |

此操作仅会更改你的账户密码。你的加密密钥不受影响，其他机器也仍会保持登录状态。

### `atuin account delete`

```shell
atuin account delete
```

删除你的账户以及服务器上所有已同步的历史记录。

| 标志 | 说明 |
|------|-------------|
| `--password`/`-p` | 你的密码。省略后 Atuin 会提示输入 |
| `--totp-code`/`-t` | 你的双因素认证代码（如果账户已启用 2FA） |

!!! warning
    此操作不会要求确认，且无法撤销。你的本地历史记录不受影响——只有服务器上的副本会被删除。

### `atuin account link`

将你的 CLI 同步账户关联到你的 [Atuin Hub](https://hub.atuin.sh/) 账户。

```shell
atuin account link
```

这会打开浏览器，让你在 Hub 完成身份验证，然后关联这两个账户。如果你已经登录了
Hub，两个账户会立即完成关联。
