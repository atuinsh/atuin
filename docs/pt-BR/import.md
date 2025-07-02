# `atuin import`

O Atuin pode importar seu histórico de comandos de seus arquivos de histórico "antigos".

`atuin import auto` tentará descobrir seu shell (via $SHELL) e executar o importador correto.

Infelizmente, esses arquivos antigos não armazenam tantas informações quanto o Atuin, então nem todos os recursos estarão disponíveis para os dados importados.

# zsh

```
atuin import zsh
```

Se você definiu HISTFILE, isso deve ser selecionado! Caso contrário, você pode tentar o seguinte:

```
HISTFILE=/path/to/history/file atuin import zsh
```

Isso suporta formas simples e estendidas.

# bash

TODO
