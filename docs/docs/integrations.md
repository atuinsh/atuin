# Integrations

This page covers integrations with shell plugins and tools. For information about how Atuin's shell hooks work and troubleshooting embedded terminals (IDEs, AI coding assistants, etc.), see [Shell Integration and Interoperability](guide/shell-integration.md).

## zsh-autosuggestions

Atuin automatically adds itself as an [autosuggest strategy](https://github.com/zsh-users/zsh-autosuggestions#suggestion-strategy).

If you'd like to override this, add your own config after `"$(atuin init zsh)"` in your `.zshrc`.

## zsh-vi-mode

If you are using [Zsh Vi Mode](https://github.com/jeffreytse/zsh-vi-mode), you may want to add the following to your `.zshrc` to prevent overriding the default atuin binds:

```shell
# Append a command directly (after sourcing zvm)
zvm_after_init_commands+=(
  'eval "$(atuin init zsh)"'
)
```

## ble.sh auto-complete (Bash)

If ble.sh is available when Atuin's integration is loaded in Bash, Atuin automatically defines and registers an auto-complete source for the autosuggestion feature of ble.sh.

If you'd like to change the behavior, please overwrite the shell function `ble/complete/auto-complete/source:atuin-history` after `eval "$(atuin init bash)"` in your `.bashrc`.

If you would not like Atuin's auto-complete source, please add the following setting after `eval "$(atuin init bash)"` in your `.bashrc`:

```shell
# bashrc (after eval "$(atuin init bash)")

ble/util/import/eval-after-load core-complete '
  ble/array#remove _ble_complete_auto_source atuin-history'
```

## Embedded Terminals and IDEs

Atuin may not work out of the box in embedded terminals found in IDEs (PyCharm, VS Code, etc.) or AI coding assistants (Claude Code, etc.). This is because these tools often start non-interactive shells that don't source your shell configuration.

For solutions and workarounds, see [Shell Integration and Interoperability](guide/shell-integration.md).
