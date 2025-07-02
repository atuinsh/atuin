# Configuração

O Atuin mantém dois arquivos de configuração, armazenados em `~/.config/atuin/`. Armazenamos os dados em `~/.local/share/atuin` (a menos que substituído por XDG_*).

Você pode alterar o caminho do diretório de configuração definindo `ATUIN_CONFIG_DIR`. Por exemplo:

```
export ATUIN_CONFIG_DIR = /home/ellie/.atuin
```

## Configuração do Cliente

```
~/.config/atuin/config.toml
```

O cliente é executado na máquina do usuário, e é isso que você deve se preocupar, a menos que esteja executando o servidor.

Veja o exemplo em [config.toml](../../atuin-client/config.toml)

### `dialect`

Isso configura a forma como o comando [stats](stats.md) analisa as datas. Ele tem dois valores possíveis:

```
dialect = "uk"
```

ou

```
dialect = "us"
```

O padrão é "us".

### `auto_sync`

Configura se a sincronização automática deve ocorrer ao fazer login. O padrão é `true`.

```
auto_sync = true/false
```

### `sync_address`

O endereço do servidor de sincronização! O padrão é `https://api.atuin.sh`.

```
sync_address = "https://api.atuin.sh"
```

### `sync_frequency`

Com que frequência sincronizar automaticamente com o servidor. Isso pode ser dado em um formato "legível por humanos". Por exemplo, `10s`, `20m`, `1h`, etc. O padrão é `1h`.

Se definido como `0`, o Atuin sincronizará após cada comando. Alguns servidores podem ter limites de taxa em potencial, o que não causará problemas.

```
sync_frequency = "1h"
```

### `db_path`

O caminho para o banco de dados SQLite do Atuin. O padrão é `~/.local/share/atuin/history.db`.

```
db_path = "~/.history.db"
```

### `key_path`

O caminho para a chave de criptografia do Atuin. O padrão é `~/.local/share/atuin/key`.

```
key = "~/.atuin-key"
```

### `session_path`

O caminho para o arquivo de sessão do servidor Atuin. O padrão é `~/.local/share/atuin/session`. Isso é essencialmente apenas um token de API.

```
key = "~/.atuin-session"
```

### `search_mode`

Qual modo de pesquisa usar. O Atuin suporta os modos de pesquisa "prefix" (prefixo), "fulltext" (texto completo) e "fuzzy" (difuso). A sintaxe de pesquisa de prefixo é "query*", a sintaxe de pesquisa de texto completo é "*query*", e a sintaxe de pesquisa difusa é [descrita abaixo](#sintaxe-de-pesquisa-difusa).

A configuração padrão é "fuzzy".

### `filter_mode`

O filtro padrão a ser usado ao pesquisar.

| Modo | Descrição |
|---|---|
| global (padrão) | Pesquisa o histórico de todos os hosts, todas as sessões, todos os diretórios |
| host | Pesquisa o histórico apenas deste host |
| session | Pesquisa o histórico apenas da sessão atual |
| directory | Pesquisa o histórico apenas do diretório atual |

O modo de filtro ainda pode ser alternado via `ctrl-r`.

```
search_mode = "fulltext"
```

#### Sintaxe de Pesquisa Difusa

A sintaxe de pesquisa `fuzzy` é baseada na [sintaxe de pesquisa do fzf](https://github.com/junegunn/fzf#search-syntax).

| Conteúdo | Tipo de Correspondência | Descrição |
|---|---|---|
| `sbtrkt` | fuzzy-match | Itens que correspondem a `sbtrkt` |
| `'wild` | exact-match (entre aspas) | Itens que contêm `wild` |
| `^music` | prefix-exact-match | Itens que começam com `music` |
| `.mp3$` | suffix-exact-match | Itens que terminam com `.mp3` |
| `!fire` | inverse-exact-match | Itens que não incluem `fire` |
| `!^music` | inverse-prefix-exact-match | Itens que não começam com `music` |
| `!.mp3$` | inverse-suffix-exact-match | Itens que não terminam com `.mp3` |

Termos de barra única atuam como um operador OR. Por exemplo, a seguinte consulta corresponde a entradas que começam com `core` e terminam com `go`, `rb` ou `py`.

```
^core go$ | rb$ | py$
```

## Configuração do Servidor

`// TODO`
