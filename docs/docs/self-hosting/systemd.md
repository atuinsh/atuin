# Systemd

First, create the service unit file
[`atuin-server.service`](https://github.com/atuinsh/atuin/raw/main/systemd/atuin-server.service) at
`/etc/systemd/system/atuin-server.service` with contents like this:

```ini
[Unit]
Description=Start the Atuin server syncing service
After=network-online.target
Wants=network-online.target systemd-networkd-wait-online.service

[Service]
ExecStart=atuin server start
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

This is the official Atuin service unit file which includes a lot of hardening options to increase
security.

Next, create [`atuin-server.conf`](https://github.com/atuinsh/atuin/raw/main/systemd/atuin-server.sysusers) at
`/etc/sysusers.d/atuin-server.conf` with contents like this:

```
u atuin - "Atuin synchronized shell history"
```
This file will ensure a system user is created in the proper manner.

Afterwards, run
```sh
systemctl restart systemd-sysusers
```
to make sure the file is read. A new `atuin-server` user should then be available.

Now, you can attempt to run the Atuin server:
```sh
systemctl enable --now atuin-server
```

```sh
systemctl status atuin-server
```

If it started fine, it should have created the default config inside `/etc/atuin/`.
