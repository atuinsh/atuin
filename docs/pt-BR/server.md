# `atuin server`

O Atuin permite que você execute seu próprio servidor de sincronização, caso não queira usar o servidor que eu (ellie) hospedo :)

Atualmente, há apenas um subcomando, `atuin server start`, que iniciará o servidor de sincronização HTTP do Atuin.

```
USO:
    atuin server start [OPÇÕES]

FLAGS:
        --help       Imprime informações de ajuda
    -V, --version    Imprime informações da versão

OPÇÕES:
    -h, --host <host>
    -p, --port <port>
```

## Configuração

A configuração do servidor é separada da configuração do cliente, mesmo que sejam o mesmo binário. A configuração do servidor pode ser encontrada em `~/.config/atuin/server.toml`.

Ela se parece com isto:

```toml
host = "0.0.0.0"
port = 8888
open_registration = true
db_uri="postgres://user:password@hostname/database"
```

Alternativamente, a configuração também pode ser fornecida por variáveis de ambiente.

```sh
ATUIN_HOST="0.0.0.0"
ATUIN_PORT=8888
ATUIN_OPEN_REGISTRATION=true
ATUIN_DB_URI="postgres://user:password@hostname/database"
```

### host

O endereço no qual o servidor Atuin deve escutar.

O padrão é `127.0.0.1`.

### port

A porta na qual o servidor Atuin deve escutar.

O padrão é `8888`.

### open_registration

Se `true`, o Atuin aceitará novos registros de usuários. Se você não quiser que outras pessoas possam usar seu servidor, defina isso como `false` depois de criar sua própria conta.

O padrão é `false`.

### db_uri

Um URI postgres válido, onde os dados do usuário e do histórico serão salvos.

### path

`path` refere-se ao prefixo de rota adicionado ao servidor. Um valor de string vazia não adicionará um prefixo de rota.

O padrão é `""`.

## Notas de Implantação em Contêiner

Você pode implantar seu próprio servidor Atuin em um contêiner:

* Para um exemplo de configuração Docker, consulte [docker](docker.md).
* Para um exemplo de configuração Kubernetes, consulte [k8s](k8s.md).
