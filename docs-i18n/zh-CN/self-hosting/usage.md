# 使用自托管服务器

!!! warning
    如果你正在自托管，我们强烈建议只使用带标签的发行版本，不要跟随 `main` 或 `latest`。

    请关注 GitHub 发行版，并阅读每个版本的发行说明。大多数情况下，升级无需任何手动干预即可完成。

    但我们无法保证所有更新都能顺利应用，其中部分更新可能需要额外的操作步骤。

## 客户端设置

要让 Atuin 使用自托管服务器，你需要在配置文件 `~/.config/atuin/config.toml` 中设置 `sync_address`。关于如何设置 `sync_address` 的更多详情，请参阅[配置](../configuration/config.md#sync_address)页面。

你也可以将环境变量 `ATUIN_SYNC_ADDRESS` 设置为正确的主机地址，例如 `ATUIN_SYNC_ADDRESS=https://api.atuin.sh`。
