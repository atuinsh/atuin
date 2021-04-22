# `atuin stats`

Atuin can also calculate stats based on your history - this is currently a
little basic, but more features to come

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

$ atuin stats day 01/01/21 # also accepts absolute dates
```

It can also calculate statistics for all of known history:

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
