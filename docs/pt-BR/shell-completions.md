# `atuin gen-completions`

As [conclusões de shell](https://en.wikipedia.org/wiki/Command-line_completion) do Atuin podem ser geradas usando o subcomando `gen-completions`, especificando o diretório de saída e o shell desejado.

```
$ atuin gen-completions --shell bash --out-dir $HOME

Conclusão de shell para BASH é gerada em "/home/user"
```

Os valores possíveis para o parâmetro `--shell` são:

- `bash`
- `fish`
- `zsh`
- `powershell`
- `elvish`

Além disso, consulte [Shells Suportados](./README.md#shells-suportados).
