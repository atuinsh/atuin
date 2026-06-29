# `atuin sync`

Atuin 可以将您的历史记录备份到服务器，并使用它来确保多台机器具有相同的 shell 历史记录。 这都是端到端加密的，因此服务器操作员_永远_看不到您的数据！

任何人都可以托管一个服务器（尝试 `atuin server start`，更多文档将在后面介绍），但我(ellie)在 https://api.atuin.sh 上托管了一个。这是默认的服务器地址，可以在 [配置](config.md) 中更改。 同样，我_不能_看到您的数据，也不想。

## 同步频率

除非另有配置，否则同步将自动执行。同步的频率可在 [配置](config.md) 中配置。

## 同步

你可以通过 `atuin sync` 来手动触发同步

## 注册

注册一个同步账号

```
atuin register -u <USERNAME> -e <EMAIL> -p <PASSWORD>
```

用户名（USERNAME）必须是唯一的，电子邮件（EMAIL）仅用于重要通知（安全漏洞、服务更改等）

注册后，意味着你也已经登录了 :) 同步应该从这里自动发生！

## 密钥

由于你的数据是加密的， Atuin 将为你生成一个密钥。它被存储在 Atuin 的数据目录里（ Linux 上为 `~/.local/share/atuin`）

你也可以通过以下方式获得它

```
atuin key
```

千万不要跟任何人分享这个！

## 登录

如果你想登录到一个新的机器上，你需要你的加密密钥（`atuin key`）。

```
atuin login -u <USERNAME> -p <PASSWORD> -k <KEY>
```

## 登出

```
atuin logout
```
