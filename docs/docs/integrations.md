# Integrations

## zsh-autosuggestions

Atuin automatically adds itself as an [autosuggest strategy](https://github.com/zsh-users/zsh-autosuggestions#suggestion-strategy).

If you'd like to override this, add your own config after "$(atuin init zsh)" in your zshrc.

## ble.sh auto-complete (Bash)

If ble.sh is available when Atuin's integration is loaded in Bash, Atuin automatically defines and registers an auto-complete source for the autosuggestion feature of ble.sh.

If you'd like to change the behavior, please overwrite the shell function `ble/complete/auto-complete/source:atuin-history` after `eval "$(atuin init bash)"` in your `.bashrc`.

If you would not like Atuin's auto-complete source, please add the following setting after `eval "$(atuin init bash)"` in your `.bashrc`:

```shell
# bashrc (after eval "$(atuin init bash)")

ble/util/import/eval-after-load core-complete '
  ble/array#remove _ble_complete_auto_source atuin-history'
```
