# `atuin search`

```
atuin search <query>
```

Pretraga u Atuin-u takođe podržava wildcards sa znacima `*` ili `%`.
Zadano, mora biti naveden prefiks (tj. svi upiti se automatski dopunjuju wildcard-ovima)

| Argument           | Opis                                                                                         |
| ------------------ | -------------------------------------------------------------------------------------------- |
| `--cwd/-c`         | Direktorij za koji se prikazuje istorija (zadano svi direktoriji)                 |
| `--exclude-cwd`    | Isključuje naredbe koje su pokrenute u ovom direktoriju (zadano none)               |
| `--exit/-e`        | Filtriranje po exit code (zadano none)                                                |
| `--exclude-exit`   | Isključuje naredbe koje su završene sa navedenom vrednošću (zadano none)              |
| `--before`         | Uključuje samo naredbe koje su pokrenute pre navedenog vremena (zadano none)          |
| `--after`          | Uključuje samo naredbe koje su pokrenute posle navedenog vremena (zadano none)        |
| `--interactive/-i` | Otvara interaktivni grafički interfejs za pretragu (zadano false)                     |
| `--human`          | Koristi čitljivo formatiranje za vreme i vremenske periode (zadano false)             |

## Primeri

```
# Pokreće interaktivnu pretragu sa tekstualnim korisničkim interfejsom
atuin search -i

# Pokreće interaktivnu pretragu sa tekstualnim korisničkim interfejsom i već unetim upitom
atuin search -i atuin

# Pretražuje sve naredbe koje počinju sa cargo i koje su uspešno završene
atuin search --exit 0 cargo

# Pretražuje sve naredbe koje su završene sa greškom, pokrenute u trenutnom direktoriju i pre prvog aprila 2021
atuin search --exclude-exit 0 --before 01/04/2021 --cwd .

# Pretražuje sve naredbe koje počinju sa cargo, uspešno završene i pokrenute posle tri sata popodne juče
atuin search --exit 0 --after "yesterday 3pm" cargo
```
