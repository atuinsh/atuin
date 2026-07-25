# daemon

## `atuin daemon`

Atuin 守护进程是一个在后台运行的进程，旨在：

1. 加快数据库写入速度
2. 让机器在闲置时也能完成同步，随时保持可用状态
3. 在内存中维护一个随时就绪的模糊搜索器
4. 执行后台维护工作

此外，它也有助于规避 ZFS 与 SQLite 之间的性能问题。

## 启用方法

在 Atuin 配置文件末尾添加以下内容：

```toml
[daemon]
enabled = true
autostart = true
```

启用 `autostart = true` 后，CLI 会自动启动并管理本地守护进程，用于处理历史记录钩子调用。
如果你使用 systemd 的 socket 激活方式，请保持 `autostart = false`。
如果你已经在运行旧版的实验性守护进程，需要手动重启一次，因为 autostart 无法对其进行原地升级。

如果你更倾向于自行运行守护进程（例如通过 systemd/tmux），请保持 `autostart = false` 并运行 `atuin daemon`。

## 更多配置

请参阅[配置章节](../configuration/config.md#daemon)。
