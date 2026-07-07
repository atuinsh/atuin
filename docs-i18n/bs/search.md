# `atuin search`

```
atuin search <query>
```

Pretraga u Atuin-u takođe podržava wildcards sa znacima `*` ili `%`.
Podrazumevano, mora biti naveden prefiks (tj. svi upiti se automatski dopunjuju wildcard-ovima)

| Argument           | Opis                                                                                         |
| ------------------ | -------------------------------------------------------------------------------------------- |
| `--cwd/-c`         | Direktorijum za koji se prikazuje istorija (podrazumevano svi direktorijumi)                 |
| `--exclude-cwd`    | Isključuje komande koje su pokrenute u ovom direktorijumu (podrazumevano none)               |
| `--exit/-e`        | Filtriranje po exit code (podrazumevano none)                                                |
| `--exclude-exit`   | Isključuje komande koje su završene sa navedenom vrednošću (podrazumevano none)              |
| `--before`         | Uključuje samo komande koje su pokrenute pre navedenog vremena (podrazumevano none)          |
| `--after`          | Uključuje samo komande koje su pokrenute posle navedenog vremena (podrazumevano none)        |
| `--interactive/-i` | Otvara interaktivni grafički interfejs za pretragu (podrazumevano false)                     |
| `--human`          | Koristi čitljivo formatiranje za vreme i vremenske periode (podrazumevano false)             |

## Primeri

```
# Pokreće interaktivnu pretragu sa tekstualnim korisničkim interfejsom
atuin search -i

# Pokreće interaktivnu pretragu sa tekstualnim korisničkim interfejsom i već unetim upitom
atuin search -i atuin

# Pretražuje sve komande koje počinju sa cargo i koje su uspešno završene
atuin search --exit 0 cargo

# Pretražuje sve komande koje su završene sa greškom, pokrenute u trenutnom direktorijumu i pre prvog aprila 2021
atuin search --exclude-exit 0 --before 01/04/2021 --cwd .

# Pretražuje sve komande koje počinju sa cargo, uspešno završene i pokrenute posle tri sata popodne juče
atuin search --exit 0 --after "yesterday 3pm" cargo
```
