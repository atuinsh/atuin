# 卸载 Atuin

很遗憾看到你离开！

如果你是使用 Atuin 安装脚本完成安装的，可以通过删除以下内容将其彻底卸载：

1. 删除 `~/.atuin` 目录
2. 删除 `~/.config/atuin` 目录
3. 删除 `~/.local/share/atuin` 目录
4. 从 shell 配置文件中删除调用 `atuin init` 的那一行
5. Fish 用户：若 `~/.config/fish/conf.d/atuin.env.fish` 存在，请将其删除

否则，卸载 Atuin 的具体方式取决于你所使用的系统，以及当初安装它的方式。

例如，在 macOS 上，你可以运行：

```shell
brew uninstall atuin
```

然后再移除 shell 集成即可。
