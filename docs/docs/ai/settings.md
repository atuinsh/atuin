# AI Settings

All the settings that control the behavior of [Atuin AI](./introduction.md) are specified in an `[ai]` section in your `config.toml`. See [the configuration documentation](../../configuration/config/) for more detailed information about Atuin's configuration system.

### enabled

Default: `false`

Whether or not the AI feature are enabled. When set to `false`, the question mark keybinding will output a message with instructions to run `atuin setup` to enable the feature.

### send_cwd

Default: `false`

Whether or not to include your current working directory in the context sent to the LLM. By default, only your OS and current shell are sent.

**Example config**

```toml
[ai]
send_cwd = true
```

### endpoint

Default: `null`

The address of the Atuin AI endpoint. Used for AI features like command generation. Most users will not need this setting; it is only necessary for custom AI endpoints.

### api_token

Default: `null`

The API token for the Atuin AI endpoint. Used for AI features like command generation. Most users will not need this setting; it is only necessary for custom AI endpoints.
