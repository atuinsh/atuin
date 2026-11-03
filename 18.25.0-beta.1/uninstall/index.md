# Uninstalling Atuin

Sorry to see you go!

If you used the Atuin installer, you can totally delete it by following these steps:

1. Delete the `~/.atuin` directory (and `$ATUIN_HOME`, if set).
1. Delete the `~/.config/atuin` and `~/.local/share/atuin` directories if they exist. Older versions of Atuin used to store data there.
1. On macOS, remove the daemon's launchd agent:

```
launchctl bootout gui/$(id -u)/sh.atuin.daemon
rm -f ~/Library/LaunchAgents/sh.atuin.daemon.plist
```

1. Remove the shell integration:
1. On Bash, remove the lines referencing `atuin init` and `~/.atuin/bin/env` from ~/.profile, ~/.bashrc, and ~/.bash_login. The installer may have installed bash-preexec; if you wish to remove it too, delete ~/.bash-preexec.sh and remove the line referencing it from ~/.bashrc.
1. On Zsh, remove the lines referencing `atuin init` and `~/.atuin/bin/env` from ~/.zshrc and ~/.zshenv.
1. On Fish, remove the line referencing `atuin init` from ~/.config/fish/config.fish and delete ~/.config/fish/conf.d/atuin.env.fish.

Otherwise, uninstalling Atuin depends on your system, and how you installed it.

For example, on macOS, you'd want to run

```
brew uninstall atuin
```

and then remove the shell integration.
