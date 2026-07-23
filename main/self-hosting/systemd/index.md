# Systemd

Note

These instructions assume the `atuin-server` binary is on your `PATH`. Since v18.12.0, the server is distributed as a separate binary — install it from the [releases page](https://github.com/atuinsh/atuin/releases) (see [Server setup](https://docs.atuin.sh/self-hosting/server-setup/index.md) for the installer).

First, create the service unit file [`atuin-server.service`](https://github.com/atuinsh/atuin/raw/main/systemd/atuin-server.service) at `/etc/systemd/system/atuin-server.service` with contents like this:

```
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

# Hardening options
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

This is the official Atuin service unit file which includes a lot of hardening options to increase security.

Next, create [`atuin-server.conf`](https://github.com/atuinsh/atuin/raw/main/systemd/atuin-server.sysusers) at `/etc/sysusers.d/atuin-server.conf` with contents like this:

```
u atuin - "Atuin synchronized shell history"
```

This file will ensure a system user is created in the proper manner.

Afterwards, run

```
systemctl restart systemd-sysusers
```

to make sure the file is read. A new `atuin` user should then be available.

Now, you can attempt to run the Atuin server:

```
systemctl enable --now atuin-server
```

```
systemctl status atuin-server
```

If it started fine, it should have created the default config inside `/etc/atuin/`.
