# Uninstalling Atuin

Sorry to see you go!

If you used the Atuin installer, you can totally delete it by following these steps:

1. Before you delete anything, take Atuin out of your shells, the daemon, and your agents:

```
atuin setup --only --no-shell-history --no-daemon --no-agent-history --no-mcp --no-threads
```

Setup removes the `atuin init` lines that it wrote to your shell's config files, such as `.zshrc`, and lists any other lines that load Atuin. Setup also stops the daemon, removes the daemon's launchd agent on macOS, and takes Atuin's hooks and MCP server out of your agents.

Setup takes Atuin's MCP server and plugin out of Hermes through the `hermes` command. If `hermes` isn't on your `PATH`, remove the `atuin` entry under `mcp_servers` in Hermes's `config.yaml` yourself. By default, Hermes keeps that file in `~/.hermes`.

1. Delete the `~/.atuin` directory (and `$ATUIN_HOME`, if set).
1. Delete the `~/.config/atuin` and `~/.local/share/atuin` directories if they exist. Older versions of Atuin used to store data there.
1. On macOS, setup's `--no-daemon` removes the daemon's launchd agent. If setup couldn't remove it, remove it yourself:

```
launchctl bootout gui/$(id -u)/sh.atuin.daemon
rm -f ~/Library/LaunchAgents/sh.atuin.daemon.plist
```

1. Remove the rest of the shell integration, which the installer wrote:
1. On Bash, remove the lines referencing `~/.atuin/bin/env` from ~/.profile, ~/.bashrc, and ~/.bash_login, and any line referencing `atuin init` that setup listed. The installer may have installed bash-preexec; if you wish to remove it too, delete ~/.bash-preexec.sh and remove the line referencing it from ~/.bashrc.
1. On Zsh, remove the lines referencing `~/.atuin/bin/env` from ~/.zshrc and ~/.zshenv, and any line referencing `atuin init` that setup listed.
1. On Fish, delete ~/.config/fish/conf.d/atuin.env.fish, and remove any line referencing `atuin init` that setup listed from ~/.config/fish/config.fish.

Otherwise, uninstalling Atuin depends on your system, and how you installed it. Run the `atuin setup` command from step 1 first, then uninstall the package.

For example, on macOS, you'd want to run

```
brew uninstall atuin
```

and then remove the shell integration.
