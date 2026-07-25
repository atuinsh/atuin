# import

## `atuin import`

Atuin 可以从你「旧」的历史记录文件中导入历史记录。

`atuin import auto` 会尝试通过 \$SHELL 判断你使用的 shell，并运行相应的导入器。

遗憾的是，这些旧格式存储的信息不如 Atuin 丰富，因此导入后的数据无法使用 Atuin 的全部功能。

除非另有说明，你都可以通过设置 `HISTFILE` 环境变量来控制要读取的文件；如果不设置，每个导入器都会尝试一些默认文件名。

```shell
HISTFILE=/path/to/history/file atuin import zsh
```

请注意，对于 Xonsh 这类将历史记录存储在多个文件而非单个文件中的 shell，`$HISTFILE` 应设置为存放这些文件的目录。

对于不存储时间戳的格式，时间戳将从当前时间开始生成，历史记录中每多一条命令，时间戳就增加 1ms。

大多数导入器都会丢弃其中含有无效 UTF-8 编码的命令。

## bash

这会从 `$HISTFILE` 或 `$HOME/.bash_history` 读取历史记录。

如果检测到时间戳顺序错乱，会发出警告；当历史记录文件开头没有时间戳、但后面的条目包含时间戳时，也可能出现这种情况。

## fish

fish 支持多个历史记录会话，因此导入器默认使用 `fish` 会话，除非设置了 `fish_history` 环境变量。要读取的文件是 `$XDG_DATA_HOME/fish/`（或 `$HOME/.local/share/fish`）目录下的 `{session}_history`。

fish 历史记录中的数据并非全部会被保留：其中一些与每条命令所用文件名相关的数据不会被 Atuin 使用，因此会被丢弃。

## nu

此导入器从 Nushell 的文本历史记录格式中读取，该格式存储在 `$XDG_CONFIG_HOME/nushell/history.txt` 或 `$HOME/.config/nushell/history.txt` 中。文件名无法另行设置。

## nu-hist-db

此导入器从 Nushell 的 SQLite 历史记录数据库中读取，该数据库存储在 `$XDG_CONFIG_HOME/nushell/history.sqlite3` 或 `$HOME/.config/nushell/history.sqlite3` 中。文件名无法另行设置。

## `powershell`

此导入器从
[PowerShell 的历史记录文件](https://learn.microsoft.com/en-us/powershell/module/psreadline/about/about_psreadline#command-history)读取。
在 Windows 上，该文件位于
`$APPDATA\Microsoft\Windows\PowerShell\PSReadLine\ConsoleHost_history.txt`。
在其他系统上，它位于
`$XDG_DATA_HOME/powershell/PSReadLine/ConsoleHost_history.txt`
或 `$HOME/.local/share/powershell/PSReadLine/ConsoleHost_history.txt`。

## replxx

[replxx](https://github.com/AmokHuginnsson/replxx) 导入器会从
`$HISTFILE` 或 `$HOME/.histfile` 读取。

## resh

[RESH](https://github.com/curusarn/resh) 导入器会从 `$HISTFILE`
或 `$HOME/.resh_history.json` 读取。

## xonsh

Xonsh 导入器会读取在 Xonsh 历史记录目录中找到的所有 JSON 文件。该目录的位置按以下方式确定：
* 如果设置了 `$HISTFILE`，则使用它的值作为历史记录目录。
* 如果设置了 `$XONSH_DATA_DIR`（如果导入器是从 Xonsh 内部调用的，通常会设置此变量），则使用 `$XONSH_DATA_DIR/history_json`。
* 如果设置了 `$XDG_DATA_HOME`，则使用 `$XDG_DATA_HOME/xonsh/history_json`。
* 否则，使用 `$HOME/.local/share/xonsh/history_json`。

Xonsh 历史记录 JSON 文件中的数据并非全部会被 Atuin 使用：Xonsh 会存储每个会话启动时的环境变量，但这部分数据会被 Atuin 丢弃；Xonsh 还可以选择性地存储每条命令的输出，如果存在，这部分数据同样会被 Atuin 忽略。

## `xonsh-sqlite`

Xonsh SQLite 导入器会从 Xonsh SQLite 历史记录文件中读取。该历史记录文件的位置确定方式与常规 Xonsh 导入器相同，只是将 `history_json` 替换为 `xonsh-history.sqlite`。

Xonsh SQLite 后端不存储环境变量，但与 JSON 后端一样，它也可以选择性地存储每条命令的输出；如果存在，这部分数据同样会被 Atuin 忽略。

## zsh

这会以基本格式或扩展格式，从 `$HISTFILE` 或 `$HOME/.zhistory`
或 `$HOME/.zsh_history` 读取 Zsh 历史记录。

## zsh-hist-db

这会从 `$HISTDB_FILE` 或
`$HOME/.histdb/zsh-history.db` 读取 Zsh histdb SQLite 文件。
