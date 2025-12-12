# `atuin gen-completions`

Atuin 的 [Shell 补全](https://en.wikipedia.org/wiki/Command-line_completion) 可以通过 `gen-completions` 子命令指定输出目录和所需的 shell 来生成。

```
$ atuin gen-completions --shell bash --out-dir $HOME

Shell completion for BASH is generated in "/home/user"
```

`--shell` 参数的可能值如下：

- `bash`
- `fish`
- `zsh`
- `powershell`
- `elvish`

此外, 请参阅 [支持的 Shells](./README.md#支持的-Shells).
