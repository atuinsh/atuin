# 已知问题

- SQLite 与 ZFS 搭配使用时，在某些配置下存在一些问题。由于 Atuin 使用了 SQLite，这可能导致你的 shell 变慢！我们创建了一个 [issue](https://github.com/atuinsh/atuin/issues/952) 来跟踪此问题，其中包含了一些解决方法。
- SQLite 通常也不太适合用于网络文件系统（例如 NFS）。
