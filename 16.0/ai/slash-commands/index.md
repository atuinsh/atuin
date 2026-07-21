# Atuin AI Slash Commands

Atuin AI includes a number of slash commands to facilitate controlling various aspects of Atuin AI's behavior. To access them, simply type `/` in the Atuin AI prompt, and a list of all available slash commands will appear. You can continue typing to filter the list of commands, and press `tab` to expand the selected command.

## Built-in Slash Commands

- **`/help`** - Show a list of all available slash commands, along with a brief description of each, and a link to this documentation.
- **`/new`** - Start a new session. This will clear the current session's context and history and start fresh.
- **`/reload`** - Reloads cached `TERMINAL.md` files on the next request - run this when you change a `TERMINAL.md` file mid-session to ensure the LLM has the latest context.
- **`/model`** - Select a different model for the current session. This will override the default model specified in your Atuin config. You can see a list of available models by running this command.

## Slash Commands from Skills

Any [user-defined skills](https://docs.atuin.sh/ai/skills/index.md) with a name will register that name as a slash command, with the skill's description as the slash command description (shown in `/help` and in the slash command fuzzy picker). For example, if you have a skill named `my-skill`, you can invoke it by typing `/my-skill` in the Atuin AI prompt. If the skill has any parameters, you can provide them after the skill name, separated by spaces.
