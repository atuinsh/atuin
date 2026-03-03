# scripts

Manage reusable, synced shell scripts. See the [Scripts guide](../guide/scripts.md) for a walkthrough.

## `scripts new`

Create a new script.

| Arg | Description |
| -------------------- | --------------------------------------------------------------------------- |
| `name` (positional)  | Name for the new script (must be unique)                                    |
| `-d`, `--description`| Description of the script                                                  |
| `-t`, `--tags`       | Tags for organizing scripts (can be specified multiple times)               |
| `--shebang`          | Custom shebang line (defaults to `#!/usr/bin/env bash`)                     |
| `--script`           | Path to a file to load as the script content                                |
| `--last [N]`         | Use the last N commands from history as the script content (default: 1)     |
| `--no-edit`          | Skip opening the editor                                                     |

### Examples

```
# Create a script interactively via your editor
atuin scripts new my-script

# Create from the last 3 history commands
atuin scripts new deploy --last 3

# Create from a file with tags and description
atuin scripts new backup --script ./backup.sh -d "Daily backup" -t ops -t cron

# Create from stdin
echo 'echo hello' | atuin scripts new greeting
```

## `scripts run`

Execute a script. If the script contains template variables, you will be prompted for any values not provided via `-v`.

| Arg | Description |
| -------------------- | --------------------------------------------------------------------------- |
| `name` (positional)  | Name of the script to run                                                   |
| `-v`, `--var`        | Template variable in `KEY=VALUE` format (can be specified multiple times)    |

### Examples

```
# Run a script
atuin scripts run deploy

# Run with template variables
atuin scripts run deploy -v env=prod -v region=us-east-1
```

## `scripts list`

List all available scripts with their tags and descriptions.

Alias: `scripts ls`

### Examples

```
atuin scripts list
atuin scripts ls
```

## `scripts get`

Display details of a script.

| Arg | Description |
| -------------------- | --------------------------------------------------------------------------- |
| `name` (positional)  | Name of the script                                                          |
| `-s`, `--script`     | Display only the executable script content with shebang                     |

### Examples

```
# View full script details in YAML format
atuin scripts get deploy

# Output just the executable script
atuin scripts get deploy -s
```

## `scripts edit`

Modify an existing script. Opens your editor by default.

| Arg | Description |
| -------------------- | --------------------------------------------------------------------------- |
| `name` (positional)  | Name of the script to edit                                                  |
| `-d`, `--description`| Update the description                                                     |
| `-t`, `--tags`       | Replace all existing tags with these new tags                               |
| `--no-tags`          | Remove all tags from the script                                             |
| `--rename`           | Rename the script                                                           |
| `-s`, `--shebang`    | Update the shebang line                                                     |
| `--script`           | Load updated content from a file                                            |
| `--no-edit`          | Skip opening the editor                                                     |

### Examples

```
# Edit script content in your editor
atuin scripts edit deploy

# Update description without opening editor
atuin scripts edit deploy -d "New deploy process" --no-edit

# Rename a script
atuin scripts edit deploy --rename deploy-v2 --no-edit

# Replace tags
atuin scripts edit deploy -t infra -t ci --no-edit
```

## `scripts delete`

Delete a script. Prompts for confirmation unless `-f` is provided.

Alias: `scripts rm`

| Arg | Description |
| -------------------- | --------------------------------------------------------------------------- |
| `name` (positional)  | Name of the script to delete                                                |
| `-f`, `--force`      | Skip confirmation prompt                                                    |

### Examples

```
# Delete with confirmation prompt
atuin scripts delete deploy

# Force delete without confirmation
atuin scripts delete deploy -f
atuin scripts rm deploy -f
```
