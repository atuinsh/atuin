# `atuin sync`

Atuin može da napravi rezervnu kopiju vaše istorije na serveru kako bi obezbedio korišćenje
iste istorije na različitim računalima. Celokupna istorija će biti šifrirana dvostranim šifriranjem,
tako da server _nikada_ neće dobiti vaše podatke!

Možete pokrenuti sopstveni server (pokretanjem `atuin server start`, o tome je napisano u drugim
datotekama dokumentacije), ali postoji i zadani na https://api.atuin.sh. To je zadana adresa servera
koja može biti promenjena u [konfiguraciji](config.md). Još jednom, ja _ne mogu_ da pristupim vašim podacima
i oni mi nisu potrebni.

## Učestalost sinkronizacije

Sinkronizacija će se odvijati automatski, ukoliko suprotno nije navedeno u konfiguraciji.
Ovaj parametar možete podesiti u [konfiguraciji](config.md)

## Sinkronizacija

Sinkronizaciju takođe možete pokrenuti ručno, koristeći naredbu `atuin sync`

## Registracija

Možete registrovati nalog za sinkronizaciju:

```
atuin register -u <USERNAME> -e <EMAIL> -p <PASSWORD>
```

Korisnička imena moraju biti jedinstvena, a elektronska pošta se koristi
samo za hitna obaveštenja (promene politika, bezbednosni incidenti itd.)

Nakon registracije, već ste prijavljeni na svoj nalog :) Od tog trenutka sinkronizacija
će se odvijati automatski

## Ključ

Pošto se svi podaci šifriraju, Atuin će prilikom rada generisati vaš ključ. On će biti sačuvan u
direktoriju sa podacima Atuin-a (`~/.local/share/atuin` na GNU/Linux sistemima)

Takođe možete to uraditi sami:

```
atuin key
```

Nikada ne dajte nikome ovaj ključ!

## Prijavljivanje

Ako želite da se prijavite sa drugog računala, biće vam potreban bezbednosni ključ (`atuin key`).

```
atuin login -u <USERNAME> -p <PASSWORD> -k <KEY>
```

## Odjavljivanje

```
atuin logout
```
