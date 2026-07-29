# Integrations

This page covers integrations with shell plugins and tools. For information about how Atuin's shell hooks work and troubleshooting embedded terminals (for example, IDEs and AI coding assistants), see [Shell Integration and Interoperability](guide/shell-integration.md).

## zsh-autosuggestions

Atuin automatically adds itself as an [autosuggest strategy](https://github.com/zsh-users/zsh-autosuggestions#suggestion-strategy).

If you'd like to override this, add your own config after `"$(atuin init zsh)"` in your `.zshrc`.

## zsh-vi-mode

If you are using [Zsh Vi Mode](https://github.com/jeffreytse/zsh-vi-mode), you may want to add the following to your `.zshrc` to prevent overriding the default Atuin binds:

```shell
# Append a command directly (after sourcing zvm)
zvm_after_init_commands+=(
  'eval "$(atuin init zsh)"'
)
```

## ble.sh autocomplete (Bash)

If ble.sh is available when Bash loads the Atuin integration, Atuin registers an autocomplete source for the autosuggestion feature of ble.sh.

If you'd like to change the behavior, please overwrite the shell function `ble/complete/auto-complete/source:atuin-history` after `eval "$(atuin init bash)"` in your `.bashrc`.

If you would not like Atuin's autocomplete source, please add the following setting after `eval "$(atuin init bash)"` in your `.bashrc`:

```shell
# bashrc (after eval "$(atuin init bash)")

ble/util/import/eval-after-load core-complete '
  ble/array#remove _ble_complete_auto_source atuin-history'
```

## Embedded Terminals and IDEs

Atuin does not always work immediately in embedded terminals. Examples are the terminals in IDEs such as PyCharm and VS Code, and in AI coding assistants such as Claude Code. These tools often start non-interactive shells, which do not source your shell configuration.

For solutions and workarounds, see [Shell Integration and Interoperability](guide/shell-integration.md).
