# 在 systemd 下运行 Atuin 服务器

!!! note
    以下说明假设 `atuin-server` 二进制文件已在你的 `PATH` 中。自 v18.12.0 起，服务器以独立二进制文件的形式分发——请从
    [发布页面](https://github.com/atuinsh/atuin/releases) 安装它（安装步骤见[服务器设置](./server-setup.md)）。

首先，创建服务单元文件
[`atuin-server.service`](https://github.com/atuinsh/atuin/raw/main/systemd/atuin-server.service)，
路径为 `/etc/systemd/system/atuin-server.service`，内容类似如下：

```ini
[Unit]
Description=Start the Atuin server syncing service
After=network-online.target
Wants=network-online.target systemd-networkd-wait-online.service

[Service]
ExecStart=atuin-server start
Restart=on-failure
User=atuin
Group=atuin

Environment=ATUIN_CONFIG_DIR=/etc/atuin
ReadWritePaths=/etc/atuin

# 加固选项
CapabilityBoundingSet=
AmbientCapabilities=
NoNewPrivileges=true
ProtectHome=true
ProtectSystem=strict
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
PrivateTmp=true
PrivateDevices=true
LockPersonality=true

[Install]
WantedBy=multi-user.target
```

这是官方提供的 Atuin 服务单元文件，其中包含了许多加固选项以提升安全性。

接下来，创建 [`atuin-server.conf`](https://github.com/atuinsh/atuin/raw/main/systemd/atuin-server.sysusers)，
路径为 `/etc/sysusers.d/atuin-server.conf`，内容类似如下：

```
u atuin - "Atuin synchronized shell history"
```
该文件可确保系统用户以正确的方式创建。

之后，运行
```sh
systemctl restart systemd-sysusers
```
以确保该文件被读取。此时应该就能看到 `atuin` 用户已经可用了。

现在，你可以尝试运行 Atuin 服务器：
```sh
systemctl enable --now atuin-server
```

```sh
systemctl status atuin-server
```

如果启动顺利，它应该已经在 `/etc/atuin/` 目录下创建了默认配置。
