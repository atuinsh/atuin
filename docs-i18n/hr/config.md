# Konfiguracija

Atuin koristi dvije konfiguracijske datoteke. One se nalaze u `~/.config/atuin/`. Podaci
se skladište u `~/.local/share/atuin` (ukoliko nije drugačije definirano u XDG\_\*).

Putanja do direktorija konfiguracije može biti promenjena postavljanjem
parametra `ATUIN_CONFIG_DIR`. Na primer

```
export ATUIN_CONFIG_DIR = /home/ellie/.atuin
```

## Korisnička konfiguracija

```
~/.config/atuin/config.toml
```

Ova datoteka se koristi kada klijent radi na lokalnom stroju (ne na serveru).

Primer možete pogledati u [config.toml](../../atuin-client/config.toml)

### `dialect`

Ovaj parametar kontroliše kako [stats](stats.md) naredba obrađuje podatke.
Može imati jednu od dve dozvoljene vrednosti:

```
dialect = "uk"
```

ili

```
dialect = "us"
```

Zadano je "us".

### `auto_sync`

Da li se automatski sinkronizirati ako je korisnik prijavljen. Zadano je da (true)
```
auto_sync = true/false
```

### `sync_address`

Adresa servera za sinkronizaciju. Zadano je `https://api.atuin.sh`.

```
sync_address = "https://api.atuin.sh"
```

### `sync_frequency`

Koliko često se klijent sinkronizira sa serverom. Može biti navedeno u
formatu čitljivom za ljude. Na primer, `10s`, `20m`, `1h`, itd.
Zadano je `1h`

Ako je vrednost postavljena na 0, Atuin će se sinkronizirati nakon svake izvršene naredbe.
Imajte na umu da serveri mogu imati ograničenje na broj poslatih zahteva.

```
sync_frequency = "1h"
```

### `db_path`

Putanja do SQLite baze podataka. Zadano je
`~/.local/share/atuin/history.db`.

```
db_path = "~/.history.db"
```

### `key_path`

Putanja do ključa za šifriranje u Atuin-u. Zadano je
`~/.local/share/atuin/key`.

```
key = "~/.atuin-key"
```

### `session_path`

Putanja do serverske datoteke sesije u Atuin-u. Zadano je
`~/.local/share/atuin/session`. U suštini, ovo je samo API token.

```
key = "~/.atuin-session"
```

### `search_mode`

Određuje koji režim pretrage će biti korišćen. Atuin podržava "prefix",
pretragu po celom tekstu (fulltext) i nepreciznu ("fuzzy") pretragu. Režim "prefix" pretražuje
po "upit\*", "fulltext" po "\*upit\*", a "fuzzy" koristi
[sledeći](#fuzzy-search-syntax) sintaksu.

Zadano je "fuzzy"

### `filter_mode`

Filter koji se zadano koristi pri pretrazi

| Vrednost         | Opis                                                               |
|------------------|--------------------------------------------------------------------|
| global (default) | Pretražuje istoriju naredbi sa svih hostova, sesija i direktorija |
| host             | Pretražuje istoriju naredbi sa ovog hosta                           |
| session          | Pretražuje istoriju naredbi ove sesije                              |
| directory        | Pretražuje istoriju naredbi izvršenih u trenutnom direktoriju     |

Režimi pretrage mogu biti promenjeni preko ctrl-r


```
search_mode = "fulltext"
```

#### fuzzy search syntax

Režim pretrage "fuzzy" zasnovan je na
[fzf search syntax](https://github.com/junegunn/fzf#search-syntax).

| Token     | Tip poklapanja               | Opis                                     |
|-----------|------------------------------|------------------------------------------|
| `sbtrkt`  | fuzzy-match                  | Sve što se poklapa sa `sbtrkt`           |
| `'wild`   | exact-match (pod navodnicima)| Sve što sadrži `wild`                    |
| `^music`  | prefix-exact-match           | Sve što počinje sa `music`               |
| `.mp3$`   | suffix-exact-match           | Sve što se završava na `.mp3`            |
| `!fire`   | inverse-exact-match          | Sve što ne sadrži `fire`                 |
| `!^music` | inverse-prefix-exact-match   | Sve što ne počinje sa `music`            |
| `!.mp3$`  | inverse-suffix-exact-match   | Sve što se ne završava na `.mp3`         |

Znak vertikalne crte označava logičko ILI. Na primer, upit ispod vraća
sve što počinje sa `core` i završava se na `go`, `rb` ili `py`.

```
^core go$ | rb$ | py$
```

## Serverska konfiguracija

`// TODO`
