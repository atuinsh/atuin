<p align="center">
 <picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://github.com/atuinsh/atuin/assets/53315310/13216a1d-1ac0-4c99-b0eb-d88290fe0efd">
  <img alt="Texto mudando dependendo do modo. Claro: 'Tão claro!' Escuro: 'Tão escuro!'" src="https://github.com/atuinsh/atuin/assets/53315310/08bc86d4-a781-4aaa-8d7e-478ae6bcd129">
</picture>
</p>

<p align="center">
<em>Histórico de shell mágico</em>
</p>

<hr/>

<p align="center">
  <a href="https://github.com/atuinsh/atuin/actions?query=workflow%3ARust"><img src="https://img.shields.io/github/actions/workflow/status/atuinsh/atuin/rust.yml?style=flat-square" /></a>
  <a href="https://crates.io/crates/atuin"><img src="https://img.shields.io/crates/v/atuin.svg?style=flat-square" /></a>
  <a href="https://crates.io/crates/atuin"><img src="https://img.shields.io/crates/d/atuin.svg?style=flat-square" /></a>
  <a href="https://github.com/atuinsh/atuin/blob/main/LICENSE"><img src="https://img.shields.io/crates/l/atuin.svg?style=flat-square" /></a>
  <a href="https://discord.gg/Fq8bJSKPHh"><img src="https://img.shields.io/discord/954121165239115808" /></a>
  <a rel="me" href="https://hachyderm.io/@atuin"><img src="https://img.io/mastodon/follow/109944632283122560?domain=https%3A%2F%2Fhachyderm.io&style=social"/></a>
  <a href="https://twitter.com/atuinsh"><img src="https://img.shields.io/twitter/follow/atuinsh?style=social" /></a>
</p>


[English] | [Português (Brasil)]

Atuin substitui seu histórico de shell existente por um banco de dados SQLite e registra conteúdo adicional para seus comandos. Além disso, ele fornece sincronização de histórico opcional e totalmente criptografada entre máquinas via servidor Atuin.

<p align="center">
  <img src="../../demo.gif" alt="animado" width="80%" />
</p>

<p align="center">
<em>Mostra o código de saída, duração do comando, última execução e o comando executado</em>
</p>

Além da interface de pesquisa, ele também pode fazer o seguinte:

```
# Pesquisar todos os comandos `make` bem-sucedidos registrados após as 15h de ontem
atuin search --exit 0 --after "yesterday 3pm" make
```

Você pode usar o servidor que eu (ellie) hospedo, ou você pode hospedar o seu próprio! Ou simplesmente não usar o recurso de sincronização. Toda a sincronização do histórico é criptografada, então mesmo que eu quisesse, não conseguiria acessar seus dados. E eu **realmente** não quero.

## Recursos

- Interface de pesquisa de histórico de tela cheia que rebinda `up` e `ctrl-r`
- Armazena o histórico do shell usando um banco de dados sqlite
- Backup e sincronização do histórico de shell criptografado
- Tenha o mesmo histórico em diferentes terminais, diferentes sessões e diferentes máquinas
- Registra o código de saída, cwd, nome do host, sessão, duração do comando, etc.
- Calcula estatísticas, como "comandos mais usados"
- Não substitui arquivos de histórico antigos
- Salto rápido para registros anteriores via atalho <kbd>Alt-\<num\></kbd>
- Alternar modos de filtro via ctrl-r; pode pesquisar o histórico apenas da sessão atual, diretório ou globalmente

## Documentação

- [Início Rápido](#início-rápido)
- [Instalação](#instalação)
- [Importar](./import.md)
- [Configuração](./config.md)
- [Pesquisa de Histórico](./search.md)
- [Sincronização de Histórico na Nuvem](./sync.md)
- [Estatísticas de Histórico](./stats.md)
- [Executando seu próprio servidor](./server.md)
- [Vinculação de Teclas](./key-binding.md)
- [Conclusões de Shell](./shell-completions.md)

## Shells Suportados

- zsh
- bash
- fish

## Comunidade

Atuin tem uma comunidade Discord, disponível [aqui](https://discord.gg/Fq8bJSKPHh).

# Início Rápido

## Usando o servidor de sincronização padrão

Isso irá registrá-lo no servidor de sincronização padrão que eu hospedo. Tudo é criptografado de ponta a ponta, então seus segredos estão seguros!

Leia mais abaixo para uso apenas offline ou para hospedar seu próprio servidor.

```
bash <(curl https://raw.githubusercontent.com/ellie/atuin/main/install.sh)

atuin register -u <USERNAME> -e <EMAIL> -p <PASSWORD>
atuin import auto
atuin sync
```

### Usando o gráfico de atividade

Além de hospedar o servidor Atuin, há também um serviço que pode ser usado para gerar seu gráfico de atividade de uso do histórico do shell! Este recurso é inspirado no gráfico de atividade do GitHub.

Por exemplo, este é o meu:

![](https://api.atuin.sh/img/ellie.png?token=0722830c382b42777bdb652da5b71efb61d8d387)

Se você também quiser, depois de fazer login no seu servidor de sincronização, execute:

```
curl https://api.atuin.sh/enable -d $(cat ~/.local/share/atuin/session)
```

O resultado será o URL do seu gráfico de atividade. Este URL pode ser compartilhado ou incorporado, o token *não* é criptografado, apenas para evitar ataques de enumeração.

## Apenas Offline (sem sincronização)

```
bash <(curl https://raw.githubusercontent.com/ellie/atuin/main/install.sh)

atuin import auto
```

## Instalação

### Script (recomendado)

O script de instalação irá ajudá-lo a configurar, garantindo que seu shell esteja configurado corretamente. Ele também usará um dos seguintes métodos, preferindo o gerenciador de pacotes do sistema (pacman, homebrew, etc.) sempre que possível.

```
# Não execute como root, ele pedirá root se necessário.
bash <(curl https://raw.githubusercontent.com/ellie/atuin/main/install.sh)
```

Então, você pode ir diretamente para [Plugin de Shell](#shell-plugin).

### Via cargo

É melhor usar [rustup](https://rustup.rs/) para configurar a toolchain do Rust, então você pode executar o seguinte comando:

```
cargo install atuin
```

Então, você pode ir diretamente para [Plugin de Shell](#shell-plugin).

### Homebrew

```
brew install atuin
```

Então, você pode ir diretamente para [Plugin de Shell](#shell-plugin).

### MacPorts

Atuin também está disponível no [MacPorts](https://ports.macports.org/port/atuin/).

```
sudo port install atuin
```

Então, você pode ir diretamente para [Plugin de Shell](#shell-plugin).

### Pacman

Atuin está disponível no [repositório da comunidade](https://archlinux.org/packages/community/x86_64/atuin/) do Arch Linux.

```
pacman -S atuin
```

Então, você pode ir diretamente para [Plugin de Shell](#shell-plugin).

### Compilar e instalar a partir do código-fonte

```
git clone https://github.com/ellie/atuin.git
cd atuin/crates/atuin
cargo install --path .
```

Então, você pode ir diretamente para [Plugin de Shell](#shell-plugin).

## <a id="shell-plugin">Plugin de Shell</a>

Depois de instalar o binário, você precisará instalar o plugin do shell. Se você usou o script de instalação, tudo isso deve ser feito para você!

### zsh

```
echo 'eval "$(atuin init zsh)"' >> ~/.zshrc
```

Ou use um gerenciador de plugins:

```
zinit load ellie/atuin
```

### bash

Precisamos configurar alguns hooks, então primeiro você precisa instalar o bash-preexec:

```
curl https://raw.githubusercontent.com/rcaloras/bash-preexec/master/bash-preexec.sh -o ~/.bash-preexec.sh
echo '[[ -f ~/.bash-preexec.sh ]] && source ~/.bash-preexec.sh' >> ~/.bashrc
```

Então configure o Atuin:

```
echo 'eval "$(atuin init bash)"' >> ~/.bashrc
```

### fish

Adicione:

```
atuin init fish | source
```

no bloco `is-interactive` em `~/.config/fish/config.fish`.

### Fig

Através do [Fig](https://fig.io), você pode instalar o plugin de script `atuin` para zsh, bash ou fish com um clique.

<a href="https://fig.io/plugins/other/atuin" target="_blank"><img src="https://fig.io/badges/install-with-fig.svg" /></a>

## ...O que significa esse nome?

Atuin é nomeado em homenagem a "The Great A'Tuin", uma tartaruga gigante da série de livros Discworld de Terry Pratchett.

[English]: ../../README.md
[Português (Brasil)]: ./README.md
