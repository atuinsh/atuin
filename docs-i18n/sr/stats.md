# `atuin stats`

Atuin takođe može da prikaže statistiku zasnovanu na istoriji. Za sada u veoma jednostavnom obliku,
ali uskoro bi trebalo da bude više mogućnosti.

Statistika se trenutno prikazuje samo na engleskom jeziku
# TODO

```
$ atuin stats day last friday

+---------------------+------------+
| Statistic           | Value      |
+---------------------+------------+
| Most used command   | git status |
+---------------------+------------+
| Commands ran        |        450 |
+---------------------+------------+
| Unique commands ran |        213 |
+---------------------+------------+

$ atuin stats day 01/01/21 # takođe prihvata apsolutne datume
```

Takođe, može biti prikazana statistika celokupne istorije poznate Atuin-u:

```
$ atuin stats all

+---------------------+-------+
| Statistic           | Value |
+---------------------+-------+
| Most used command   |    ls |
+---------------------+-------+
| Commands ran        |  8190 |
+---------------------+-------+
| Unique commands ran |  2996 |
+---------------------+-------+
```
