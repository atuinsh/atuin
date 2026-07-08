# Uninstalling Atuin

Sorry to see you go!

If you used the Atuin installer, you can totally delete it by removing the following

1. Delete the `~/.atuin` directory
2. Delete the `~/.config/atuin` directory
3. Delete the `~/.local/share/atuin` directory
4. Remove the line referencing "atuin init" from your shell config
5. Fish users should also delete `~/.config/fish/conf.d/atuin.env.fish`

Otherwise, uninstalling Atuin depends on your system, and how you installed it.

EG, on macOS, you'd want to run

```
brew uninstall atuin
```

and then remove the shell integration.
