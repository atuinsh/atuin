# Scripts

Your shell history holds a record of your most useful, time-saving, and obscure
one-liners. Atuin already makes them easier to search and recall, but they can
still be hard to reuse or share cleanly. With Atuin Scripts, that changes.

Scripts let you turn any shell command (or series of commands) into a reusable,
synced, and shareable snippet. They use Atuin's built-in sync system, so scripts
are end-to-end encrypted and available on all of your machines.

Scripts support:

- Creating snippets from your history, a file, stdin, or your editor
- Tags and descriptions for easy organization
- Template variables powered by [minijinja](https://github.com/mitsuhiko/minijinja), with interactive prompting for missing values
- Custom shebangs for any interpreter (bash, python, etc.)
- End-to-end encrypted sync across machines

## Creating a script

The simplest way to create a script is to open your editor:

```
atuin scripts new my-script
```

This opens your `$EDITOR` (defaulting to `vi`) with an empty file. Write your
script, save, and quit — it's stored locally and synced to your Atuin account.

### From your history

You can create a script from your most recent commands with the `--last` flag:

```
atuin scripts new deploy --last 3
```

This pulls the last 3 commands from your history into your editor. Edit them as
needed, save, and quit. To skip the editor, add `--no-edit`:

```
atuin scripts new deploy --last 3 --no-edit
```

### From a file

Load a script from an existing file:

```
atuin scripts new backup --script ./backup.sh
```

### From stdin

You can also pipe content in:

```
echo 'echo hello world' | atuin scripts new greeting
```

### With metadata

Add a description and tags when creating a script:

```
atuin scripts new deploy --last 3 -d "Deploys the prod stack" -t infra -t prod
```

You can also specify a custom shebang for non-bash scripts:

```
atuin scripts new analyze -d "Run analysis" --shebang "#!/usr/bin/env python3"
```

## Running a script

Run a script by name:

```
atuin scripts run deploy
```

### Template variables

Scripts can contain [minijinja](https://github.com/mitsuhiko/minijinja) template variables. For example, a script containing:

```bash
echo "Hello, {{ name }}!"
kubectl config use-context {{ cluster }}
```

When you run this script, Atuin will prompt you for any missing values:

```
$ atuin scripts run greet
This script contains template variables that need values:
Enter value for 'name':
Enter value for 'cluster':
```

You can also provide variables on the command line:

```
atuin scripts run greet -v name=John -v cluster=prod
```

## Listing scripts

List all your scripts:

```
atuin scripts list
```

This displays each script's name, tags, and description.

## Viewing a script

View full details of a script in YAML format:

```
atuin scripts get deploy
```

To output just the executable script content (with shebang), use `-s`:

```
atuin scripts get deploy -s
```

## Editing a script

Edit a script in your editor:

```
atuin scripts edit deploy
```

You can also update metadata without opening the editor:

```
# Update the description
atuin scripts edit deploy -d "New deploy process" --no-edit

# Replace all tags
atuin scripts edit deploy -t infra -t ci --no-edit

# Remove all tags
atuin scripts edit deploy --no-tags --no-edit

# Rename a script
atuin scripts edit deploy --rename deploy-v2 --no-edit

# Update the shebang
atuin scripts edit deploy --shebang "#!/bin/bash" --no-edit

# Replace script content from a file
atuin scripts edit deploy --script ./new-deploy.sh --no-edit
```

## Deleting a script

Delete a script by name:

```
atuin scripts delete deploy
```

You will be prompted to confirm. To skip confirmation:

```
atuin scripts delete deploy -f
```

`rm` is available as an alias for `delete`:

```
atuin scripts rm deploy -f
```

## Syncing

Scripts sync automatically alongside your history when you run `atuin sync`. Like all
Atuin data, scripts are end-to-end encrypted before leaving your machine.

If you have [set up sync](sync.md), your scripts will be available across all of
your machines. See [Setting up sync](sync.md) for more details.
