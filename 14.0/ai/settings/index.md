# AI Settings

All the settings that control the behavior of [Atuin AI](https://docs.atuin.sh/ai/introduction/index.md) are specified in an `[ai]` section in your `config.toml`. See [the configuration documentation](https://docs.atuin.sh/configuration/config/index.md) for more detailed information about Atuin's configuration system.

### enabled

Default: `false`

Whether or not the AI feature are enabled. When set to `false`, the question mark keybinding will output a message with instructions to run `atuin setup` to enable the feature.

### model

Default: unset

The Atuin AI model to use for new sessions. If unset, the default model will be used. You can see the available models by running `/model` inside the Atuin AI interface.

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

### endpoint_protocol

Default: `"auto"`

How the client talks to the configured `endpoint`. One of:

- `"auto"` — infer from `endpoint`: official Atuin addresses use the Hub protocol, anything else is treated as an OSS server.
- `"hub"` — treat the endpoint as an Atuin Hub instance: log in via the browser-based Hub flow and report credit usage. Mostly useful for developing against a local Hub instance.
- `"oss"` — treat the endpoint as a standalone AI server, such as [atuin-ai-server](https://github.com/atuinsh/atuin-ai-server). No login flow; requests are authenticated with `api_token` if set.

With the default of `"auto"`, pointing `endpoint` at your own server just works: set `api_token` if your server requires one.

### yolo

Default: `false`

Enables YOLO mode, which automatically allows all permission checks. **Use this setting with caution.**

This setting does *not* enable any capabilities; it simply bypasses any permission checks.

## Capabilities

Settings that control what capabilities are sent to the LLM, which the LLM uses to understand what features the client has available. These are specified under `[ai.capabilities]`.

### enable_history_search

Default: `true`

Whether or not to include the "history search" capability in the context sent to the LLM. This allows the AI to request to search your Atuin history for relevant commands when generating suggestions or answering questions.

### enable_history_output

Default: `true`

Whether or not to include the "history output" capability in the context sent to the LLM. This allows the AI to request to view the output of previous commands. This requires the [pty-proxy](https://docs.atuin.sh/reference/pty-proxy/index.md) and [daemon](https://docs.atuin.sh/reference/daemon/index.md) to be enabled and running in order for Atuin to capture commands' outputs — see [Reading Command Output](https://docs.atuin.sh/ai/command-output/index.md) for setup.

### enable_file_tools

Default: `true`

Whether or not to include the "file tools" capability in the context sent to the LLM. This allows the AI to request to read and update files on your system.

### enable_command_execution

Default: `true`

Whether or not to include the "command execution" capability in the context sent to the LLM. This allows the AI to request to execute commands on your system.

**Example config**

```
[ai.capabilities]
enable_history_search = false
```

## Opening context

Settings that control what context is sent in the opening AI request. These are specified under `[ai.opening]`.

### send_cwd

Default: `false`

Whether or not to include your current working directory in the context sent to the LLM. By default, only your OS and current shell are sent.

**Example config**

```
[ai.opening]
send_cwd = true
```

### send_last_command

Default: `false`

Whether or not to send your previous command as context in the initial request, allowing the AI to provide more relevant suggestions.

**Example config**

```
[ai.opening]
send_last_command = true
```
