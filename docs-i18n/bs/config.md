# Konfiguracija

Atuin koristi dva konfiguraciona fajla. Oni se nalaze u `~/.config/atuin/`. Podaci
se skladište u `~/.local/share/atuin` (ukoliko nije drugačije definisano u XDG\_\*).

Putanja do direktorijuma konfiguracije može biti promenjena postavljanjem
parametra `ATUIN_CONFIG_DIR`. Na primer

```
export ATUIN_CONFIG_DIR = /home/ellie/.atuin
```

## Korisnička konfiguracija

```
~/.config/atuin/config.toml
```

Ovaj fajl se koristi kada klijent radi na lokalnoj mašini (ne na serveru).

Primer možete pogledati u [config.toml](../../atuin-client/config.toml)

### `dialect`

Ovaj parametar kontroliše kako [stats](stats.md) komanda obrađuje podatke.
Može imati jednu od dve dozvoljene vrednosti:

```
dialect = "uk"
```

ili

```
dialect = "us"
```

Podrazumevano je "us".

### `auto_sync`

Da li se automatski sinhronizovati ako je korisnik prijavljen. Podrazumevano je da (true)
```
auto_sync = true/false
```

### `sync_address`

Adresa servera za sinhronizaciju. Podrazumevano je `https://api.atuin.sh`.

```
sync_address = "https://api.atuin.sh"
```

### `sync_frequency`

Koliko često se klijent sinhronizuje sa serverom. Može biti navedeno u
formatu čitljivom za ljude. Na primer, `10s`, `20m`, `1h`, itd.
Podrazumevano je `1h`

Ako je vrednost postavljena na 0, Atuin će se sinhronizovati nakon svake izvršene komande.
Imajte na umu da serveri mogu imati ograničenje na broj poslatih zahteva.

```
sync_frequency = "1h"
```

### `db_path`

Putanja do SQLite baze podataka. Podrazumevano je
`~/.local/share/atuin/history.db`.

```
db_path = "~/.history.db"
```

### `key_path`

Putanja do ključa za šifrovanje u Atuin-u. Podrazumevano je
`~/.local/share/atuin/key`.

```
key = "~/.atuin-key"
```

### `session_path`

Putanja do serverskog fajla sesije u Atuin-u. Podrazumevano je
`~/.local/share/atuin/session`. U suštini, ovo je samo API token.

```
key = "~/.atuin-session"
```

### `search_mode`

Određuje koji režim pretrage će biti korišćen. Atuin podržava "prefix",
pretragu po celom tekstu (fulltext) i nepreciznu ("fuzzy") pretragu. Režim "prefix" pretražuje
po "upit\*", "fulltext" po "\*upit\*", a "fuzzy" koristi
[sledeći](#fuzzy-search-syntax) sintaksu.

Podrazumevano je "fuzzy"

### `filter_mode`

Filter koji se podrazumevano koristi pri pretrazi

| Vrednost         | Opis                                                               |
|------------------|--------------------------------------------------------------------|
| global (default) | Pretražuje istoriju komandi sa svih hostova, sesija i direktorijuma |
| host             | Pretražuje istoriju komandi sa ovog hosta                           |
| session          | Pretražuje istoriju komandi ove sesije                              |
| directory        | Pretražuje istoriju komandi izvršenih u trenutnom direktorijumu     |

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
