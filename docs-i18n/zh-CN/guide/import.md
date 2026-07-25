# 导入现有历史记录

Atuin 通过 shell 插件捕获新产生的 shell 历史记录，但对于此前已有的历史记录，你需要手动导入。

以下命令会导入当前 shell 的历史记录：
```shell
atuin import auto
```

你也可以显式指定要导入的 shell：

```shell
atuin import bash
atuin import zsh # 等等
```

无论你是否使用 Atuin，原有的 shell 历史记录文件都会继续正常更新。
