# AI Settings

All the settings that control the behavior of [Atuin AI](./introduction.md) are specified in an `[ai]` section in your `config.toml`. See [the configuration documentation](../../configuration/config/) for more detailed information about Atuin's configuration system.

### enabled

Default: `false`

Whether or not the AI feature are enabled. When set to `false`, the question mark keybinding will output a message with instructions to run `atuin setup` to enable the feature.

### db_path

Default: `ai_sessions.db` in the Atuin data directory.

The path to the SQLite database where Atuin AI sessions are stored.

### session_continue_minutes

Default: `60` (minutes)

The amount of time after the last interaction with Atuin AI that a session is considered "recent" and can be automatically continued. If you interact with Atuin AI and then invoke it again within this time window, the second interaction will be part of the same session. If you wait longer than this time window, a new session will be started. You can always start a new session manually by using the `/new` slash command in the Atuin AI interface.

### endpoint

Default: `null`

The address of the Atuin AI endpoint. Used for AI features like command generation. Most users will not need this setting; it is only necessary for custom AI endpoints.

### api_token

Default: `null`

The API token for the Atuin AI endpoint. Used for AI features like command generation. Most users will not need this setting; it is only necessary for custom AI endpoints.

## Capabilities

Settings that control what capabilities are sent to the LLM, which the LLM uses to understand what features the client has available. These are specified under `[ai.capabilities]`.

### enable_history_search

Default: `true`

Whether or not to include the "history search" capability in the context sent to the LLM. This allows the AI to request to search your Atuin history for relevant commands when generating suggestions or answering questions.

**Example config**

```toml
[ai.capabilities]
enable_history_search = false
```

## Opening context

Settings that control what context is sent in the opening AI request. These are specified under `[ai.opening]`.

### send_cwd

Default: `false`

Whether or not to include your current working directory in the context sent to the LLM. By default, only your OS and current shell are sent.

**Example config**

```toml
[ai.opening]
send_cwd = true
```

### send_last_command

Default: `false`

Whether or not to send your previous command as context in the initial request, allowing the AI to provide more relevant suggestions.

**Example config**

```toml
[ai.opening]
send_last_command = true
```
