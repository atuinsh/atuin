# `atuin import`

Atuin može da uveze istoriju iz "starog" fajla istorije

`atuin import auto` pokušava da odredi tip komandnog interfejsa
(preko \$SHELL) i pokreće odgovarajući skript za uvoz.

Nažalost, ovi fajlovi ne sadrže toliko informacija koliko Atuin, tako da neće
sve funkcije biti dostupne sa uvezenim podacima.

# zsh

```
atuin import zsh
```

Ako imate HISTFILE, onda bi ova komanda trebalo da funkcioniše. U suprotnom, pokušajte

```
HISTFILE=/path/to/history/file atuin import zsh
```

Ovaj parametar podržava kako pojednostavljen, tako i pun format.

# bash

TODO
