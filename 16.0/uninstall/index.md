# Uninstalling Atuin

Sorry to see you go!

If you used the Atuin installer, you can totally delete it by removing the following

1. Delete the `~/.atuin` directory
1. Delete the `~/.config/atuin` directory
1. Delete the `~/.local/share/atuin` directory
1. Remove the line referencing "atuin init" from your shell config
1. Fish users: delete `~/.config/fish/conf.d/atuin.env.fish` if it exists

Otherwise, uninstalling Atuin depends on your system, and how you installed it.

EG, on macOS, you'd want to run

```
brew uninstall atuin
```

and then remove the shell integration.
