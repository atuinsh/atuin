# `atuin gen-completions`

[Shell completions](https://en.wikipedia.org/wiki/Command-line_completion) для Atuin
могут бять сгенерированы путём указания каталога для вывода и желаемого shell через субкомманду `gen-completions`.

```
$ atuin gen-completions --shell bash --out-dir $HOME

Shell completion for BASH is generated in "/home/user"
```

Возможные команды для аргумента `--shell`могут быть следующими:

- `bash`
- `fish`
- `zsh`
- `powershell`
- `elvish`

Также рекомендуем прочитать  [supported shells](./../../README.md#supported-shells).
