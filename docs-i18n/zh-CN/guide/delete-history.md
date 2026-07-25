# 删除历史记录

Atuin 提供了多种删除历史记录的方式，无论你是想删除单条记录、按查询条件批量删除、清理重复项，还是清空全部历史记录。

所有删除方式都是本地优先的。如果你启用了同步，Atuin 会自动将删除操作同步到其他机器。

## 删除单条记录

删除单条记录最快的方式是通过交互式 TUI。

### 使用检查器（inspector）

1. 按 ++ctrl+r++ 或按上方向键打开 TUI
2. 搜索你想删除的记录
3. 按 ++ctrl+o++ 对选中的记录打开检查器
4. 确认这是你要删除的记录
5. 按 ++ctrl+d++ 将其删除

### 使用前缀快捷键

1. 按 ++ctrl+r++ 或按上方向键打开 TUI
2. 定位到你想删除的记录
3. 按 ++ctrl+a++ 再按 ++d++ 删除选中的记录

这两种方法都会立即删除记录，无需进一步确认。

## 删除匹配查询条件的记录

使用 `atuin search --delete` 可以删除所有匹配搜索查询的记录。它使用与常规搜索相同的查询语法，因此你可以在运行 `--delete` 之前先预览它会删除哪些内容。

### 先预览，再删除

务必先不带 `--delete` 运行一次查询，以确认结果：

```shell
# 第一步：预览 - 查看匹配到的内容
atuin search "^curl https://internal"

# 第二步：删除 - 在确认结果无误后执行
atuin search --delete "^curl https://internal"
```

### 组合过滤条件

你可以将 `--delete` 与任意搜索过滤条件组合使用：

```shell
# 删除在指定目录下运行失败的所有命令
atuin search --delete --exit 1 --cwd /home/user/experiments

# 删除在某个日期之前运行、且匹配某个模式的命令
atuin search --delete --before "2024-01-01" "^tmp-script"

# 删除昨天下午 3 点之后运行成功的 cargo 命令
atuin search --delete --exit 0 --after "yesterday 3pm" cargo
```

!!! warning
    `--delete` 需要指定查询或过滤条件，缺少条件时不会执行。这是有意的设计，用于防止意外的批量删除。

## 删除全部历史记录

如果你想清空整个本地历史记录：

```shell
atuin search --delete-it-all
```

!!! danger
    这会删除本地历史数据库中的每一条记录。它无法与查询或过滤条件组合使用。此操作不可撤销。

### 使用同步时的重新开始

如果你使用同步功能，并希望彻底重新开始，仅靠 `--delete-it-all` 是不够的。Atuin 的同步机制会把每一次操作（包括删除操作）都记录为一条加密记录。本地删除 10 万条记录，就会产生 10 万条删除记录，而这些记录仍然需要同步。当你的其他机器拉取这些记录时，仍要逐一处理每一条，你的数据库里也依然背负着这些历史记录带来的开销。

更干净的做法是删除你的同步账号并重新开始：

```shell
# 删除你的同步账号以及所有服务器端数据
atuin account delete

# 注册一个新账号
atuin register

# 重新导入你的 shell 历史记录（可选）
atuin import auto
```

这样服务器上不会留下任何残留记录。你的其他机器随后可以使用新账号重新注册，同样从头开始。

!!! tip
    如果你只想删除特定记录而保留其余部分，`atuin search --delete` 是合适的工具。只有当你想清空一切并重新开始时，重置账号的方式才更好。

## 清理被过滤规则匹配的命令

如果你更新了 [`history_filter`](../configuration/config.md#history_filter) 配置，并希望回溯清理匹配新过滤规则的记录：

```shell
# 预览将被移除的内容
atuin history prune --dry-run

# 执行删除
atuin history prune
```

在你为 `history_filter` 新增匹配模式时，这个功能会很有用：Atuin 此后不会再记录匹配该过滤规则的命令，但在你设置规则之前已经记录下来的旧记录依然存在，`prune` 就是用来清理这些旧记录的。

## 清除无法解密的本地存储记录

如果 `atuin store verify` 报告某些本地存储记录无法用你当前的密钥解密，你可以只移除这些损坏的本地记录：

```sh
# 检查是否每一条本地存储记录都能被解密
atuin store verify

# 只删除解密失败的本地记录
atuin store purge
```

当某台机器上存在使用其他密钥加密的本地记录时，这个功能就很有用。参阅[存储参考文档](../reference/store.md)了解其他记录存储命令。

!!! warning
    `atuin store purge` 只影响当前机器上的本地记录存储。它不会清空你的历史记录、删除你的同步账号，也不会重置其他机器。

!!! danger
    `atuin store purge` 会永久删除无法解密的记录。在运行它之前，请确认 Atuin 使用的是你打算保留的密钥，并在这些记录可能仍可恢复的情况下备份本地存储。请先运行 `atuin store verify`，以确认你清理的确实是密钥不匹配的问题，而不是在盲目删除数据。

## 去除重复的历史记录

移除重复记录（命令、工作目录、主机名均相同）：

```shell
# 预览将被移除的重复项
atuin history dedup --dry-run --before "2025-01-01" --dupkeep 1

# 删除它们
atuin history dedup --before "2025-01-01" --dupkeep 1
```

| 标志 | 描述 |
|------|-------------|
| `--dry-run`/`-n` | 列出重复项而不删除 |
| `--before`/`-b` | 只考虑在此日期之前添加的记录（必填） |
| `--dupkeep` | 保留的近期重复项数量 |

## 删除你的同步账号

要删除你的远程同步账号以及所有服务器端历史记录：

```shell
atuin account delete
```

这会移除你的账号以及服务器上所有已同步的历史记录。**本地历史记录不受影响。**更多详情请参阅[同步参考文档](../reference/sync.md)。

## 总结

| 目标 | 命令 |
|------|---------|
| 删除单条记录（TUI） | ++ctrl+o++ 然后 ++ctrl+d++，或 ++ctrl+a++ 然后 ++d++ |
| 按查询删除记录 | `atuin search --delete <query>` |
| 删除全部历史记录 | `atuin search --delete-it-all` |
| 重新开始（使用同步时） | `atuin account delete` 然后重新注册 |
| 移除被过滤的记录 | `atuin history prune` |
| 移除无法解密的本地存储记录 | 先 `atuin store verify` 再 `atuin store purge` |
| 移除重复项 | `atuin history dedup --before <date> --dupkeep <n>` |
| 删除同步账号 | `atuin account delete` |
