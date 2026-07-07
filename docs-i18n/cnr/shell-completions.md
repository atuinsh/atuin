# `atuin gen-completions`

[Dopunjavanje komandi](https://en.wikipedia.org/wiki/Command-line_completion) za Atuin
može biti generisano navođenjem direktorijuma za izlaz i željenog shell-a kroz podkomandu `gen-completions`.

```
$ atuin gen-completions --shell bash --out-dir $HOME

Shell completion for BASH is generated in "/home/user"
```

Moguće vrednosti za argument `--shell` su sledeće:

- `bash`
- `fish`
- `zsh`
- `powershell`
- `elvish`

Takođe preporučujemo da pročitate [supported shells](./../../README.md#supported-shells).
