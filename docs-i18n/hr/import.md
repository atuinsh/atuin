# `atuin import`

Atuin može da uveze istoriju iz "stare" datoteke istorije

`atuin import auto` pokušava da odredi tip naredbenog sučelja
(preko \$SHELL) i pokreće odgovarajući skript za uvoz.

Nažalost, ove datoteke ne sadrže toliko informacija koliko Atuin, tako da neće
sve funkcije biti dostupne sa uvezenim podacima.

# zsh

```
atuin import zsh
```

Ako imate HISTFILE, onda bi ova naredba trebalo da funkcioniše. U suprotnom, pokušajte

```
HISTFILE=/path/to/history/file atuin import zsh
```

Ovaj parametar podržava kako pojednostavljen, tako i pun format.

# bash

TODO
