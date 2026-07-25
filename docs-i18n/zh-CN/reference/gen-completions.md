# gen-completions

Atuin 支持通过 `gen-completions` 子命令生成 [shell 自动补全](https://en.wikipedia.org/wiki/Command-line_completion) 脚本，只需指定输出目录和所需的 shell 即可。

```console
$ atuin gen-completions --shell bash --out-dir $HOME

Shell completion for BASH is generated in "/home/user"
```

`--shell` 参数的可选值如下：

- `bash`
- `fish`
- `zsh`
- `nushell`
- `powershell`
- `elvish`

另请参阅[支持的平台](../support.md)。
