# `atuin sync`

O Atuin pode fazer backup do seu histórico para um servidor e usá-lo para garantir que várias máquinas tenham o mesmo histórico de shell. Tudo é criptografado de ponta a ponta, então o operador do servidor *nunca* pode ver seus dados!

Qualquer um pode hospedar um servidor (tente `atuin server start`, mais documentação virá depois), mas eu (ellie) hospedo um em https://api.atuin.sh. Este é o endereço do servidor padrão e pode ser alterado na [configuração](config.md). Novamente, eu *não* posso ver seus dados, nem quero.

## Frequência de Sincronização

A sincronização ocorrerá automaticamente, a menos que configurado de outra forma. A frequência da sincronização pode ser configurada na [configuração](config.md).

## Sincronizar

Você pode acionar uma sincronização manualmente com `atuin sync`.

## Registrar

Registre uma conta de sincronização.

```
atuin register -u <USERNAME> -e <EMAIL> -p <PASSWORD>
```

O NOME DE USUÁRIO deve ser único, e o EMAIL é usado apenas para notificações importantes (vulnerabilidades de segurança, alterações de serviço, etc.).

Uma vez registrado, você também estará logado :) A sincronização deve ocorrer automaticamente a partir daqui!

## Chave

Como seus dados são criptografados, o Atuin gerará uma chave para você. Ela é armazenada no diretório de dados do Atuin (em Linux, `~/.local/share/atuin`).

Você também pode obtê-la com:

```
atuin key
```

Nunca compartilhe isso com ninguém!

## Login

Se você quiser fazer login em uma nova máquina, precisará de sua chave de criptografia (`atuin key`).

```
atuin login -u <USERNAME> -p <PASSWORD> -k <KEY>
```

## Logout

```
atuin logout
```