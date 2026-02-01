# `atuin search`

```
atuin search <query>
```

A pesquisa do Atuin também suporta curingas com os caracteres `*` ou `%`. Por padrão, uma pesquisa de prefixo é executada (ou seja, todos os termos de pesquisa são automaticamente anexados com um curinga).

| Parâmetro | Descrição |
|---|---|
| `--cwd/-c` | Diretório para listar o histórico (padrão: todos os diretórios) |
| `--exclude-cwd` | Exclui comandos executados neste diretório (padrão: nenhum) |
| `--exit/-e` | Filtra por código de saída (padrão: nenhum) |
| `--exclude-exit` | Exclui comandos com este código de saída (padrão: nenhum) |
| `--before` | Inclui apenas comandos executados antes desta data (padrão: nenhum) |
| `--after` | Inclui apenas comandos executados depois desta data (padrão: nenhum) |
| `--interactive/-i` | Abre a interface de usuário de pesquisa interativa (padrão: false) |
| `--human` | Usa formato legível para humanos para carimbos de data/hora e durações (padrão: false) |

## Exemplos

```
# Abrir a TUI de pesquisa interativa
atuin search -i

# Abrir a TUI de pesquisa interativa pré-preenchida com uma consulta
atuin search -i atuin

# Pesquisar todos os comandos que começam com cargo e saíram com sucesso.
atuin search --exit 0 cargo

# Pesquisar todos os comandos que falharam, foram executados antes de 1º de abril de 2021 e no diretório atual.
atuin search --exclude-exit 0 --before 01/04/2021 --cwd .

# Pesquisar todos os comandos que começam com cargo, saíram com sucesso e foram executados após as 15h de ontem.
atuin search --exit 0 --after "yesterday 3pm" cargo
```
