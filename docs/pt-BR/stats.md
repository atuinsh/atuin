# `atuin stats`

O Atuin também pode calcular estatísticas com base no seu histórico - atualmente é apenas um recurso básico, mas mais estão por vir.

```
$ atuin stats day last friday

+---------------------+------------+
| Estatística         | Valor      |
+---------------------+------------+
| Comando mais usado  | git status |
+---------------------+------------+
| Comandos executados |        450 |
+---------------------+------------+
| Comandos únicos     |        213 |
+---------------------+------------+

$ atuin stats day 01/01/21 # Também aceita datas absolutas
```

Ele também pode calcular estatísticas para todo o histórico conhecido.

```
$ atuin stats all

+---------------------+-------+
| Estatística         | Valor |
+---------------------+-------+
| Comando mais usado  |    ls |
+---------------------+-------+
| Comandos executados |  8190 |
+---------------------+-------+
| Comandos únicos     |  2996 |
+---------------------+-------+
```
