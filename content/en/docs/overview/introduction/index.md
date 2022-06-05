---
title: "Introduction"
description: "Welcome to Atuin"
lead: ""
date: 2022-06-05T21:15:56+01:00
lastmod: 2022-06-05T21:15:56+01:00
draft: false
images: []
type: docs
weight: 1
---
Atuin replaces your existing shell history with a SQLite database, and records
additional context for your commands. Additionally, it provides optional and
fully encrypted synchronisation of your history between machines, via an Atuin
server.

As well as the search UI, it can do things like this:

```
# search for all successful `make` commands, recorded after 3pm yesterday
atuin search --exit 0 --after "yesterday 3pm" make
```


## Features

- rebind `up` and `ctrl-r` with a full screen history search UI
- store shell history in a sqlite database
- backup and sync **encrypted** shell history
- the same history across terminals, across sessions, and across machines
- log exit code, cwd, hostname, session, command duration, etc
- calculate statistics such as "most used command"
- old history file is not replaced
- quick-jump to previous items with <kbd>Alt-\<num\></kbd>
- switch filter modes via ctrl-r; search history just from the current session, directory, or globally

## Supported Shells

- zsh
- bash
- fish
 
## Community

Atuin has a community Discord, available [here](https://discord.gg/Fq8bJSKPHh)
