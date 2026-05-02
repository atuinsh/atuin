# `atuin import`

Atuin 可以从您的“旧”历史文件中导入您的历史记录

`atuin import auto` 将尝试找出你的 shell（通过 \$SHELL）并运行正确的导入器

不幸的是，这些旧文件没有像 Atuin 那样存储尽可能多的信息，因此并非所有功能都可用于导入的数据。

# zsh

```
atuin import zsh
```

如果你设置了 HISTFILE，这应该会被选中！如果没有，可以尝试以下操作

```
HISTFILE=/path/to/history/file atuin import zsh
```

这支持简单和扩展形式

# bash

TODO
