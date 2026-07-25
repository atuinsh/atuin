# sync

Atuin 可以将你的历史记录备份到服务器，确保多台机器之间共享同一份 shell 历史记录。这一切都是端到端加密的，服务器运营者*永远*无法看到你的数据！

任何人都可以自行托管服务器（参见[自托管文档](../self-hosting/server-setup.md)），我们也托管了一个，地址是 <https://api.atuin.sh>。这是默认的服务器地址，可以在[配置](../configuration/config.md#sync_address)中修改。再次强调，我们*看不到*你的数据，也不想看到。

## 同步频率

除非另行配置，否则同步会自动进行。同步频率可在[配置](../configuration/config.md#sync_frequency)中设置。

## 同步

你可以通过 `atuin sync` 手动触发一次同步。

## 注册

使用以下命令注册同步账户：

```shell
atuin register -u <USERNAME> -e <EMAIL> -p <PASSWORD>
```

如果不想让密码出现在 shell 历史记录中，可以省略密码参数，Atuin 会通过 `stdin` 询问密码。

用户名必须唯一，且只能包含字母数字或连字符；邮箱仅用于发送重要通知（例如安全漏洞、服务变更等）。

注册成功后，你也会自动登录 :) 从此同步应该会自动进行！

## 删除

你可以通过以下命令删除同步账户：

```shell
atuin account delete
```

这将删除你的账户以及服务器上所有已同步的历史记录，但不会影响本地数据！

## 密钥 {#key}

由于你的所有数据都经过加密，Atuin 会为你生成一个密钥，并将其存储在 Atuin 的数据目录中（在 Linux 上为 `~/.local/share/atuin`）。

你也可以通过以下命令获取这个密钥：

```shell
atuin key
```

切勿将其分享给任何人！

## 登录

如果你想登录到一台新机器，需要用到你的加密密钥（`atuin key`）。

```shell
atuin login -u <USERNAME> -p <PASSWORD> -k <KEY>
```

如果不想让密码或加密密钥出现在 shell 历史记录中，可以省略相应参数，Atuin 会通过 `stdin` 询问。

## 登出

```shell
atuin logout
```
