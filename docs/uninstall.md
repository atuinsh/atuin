# Uninstalling Atuin

Sorry to see you go!

If you used the Atuin installer, you can totally delete it by removing the following

1. Delete the ~/.atuin directory
2. Delete the ~/.local/share/atuin directory
3. Remove the line referencing "atuin init" from your shell config

Otherwise, uninstalling Atuin depends on your system, and how you installed it.

EG, on macOS, you'd want to run

```
brew uninstall atuin
```

and then remove the shell integration.
