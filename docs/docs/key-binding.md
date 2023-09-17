---
sidebar_position: 2
---

# Key binding

Atuin does not yet have full key binding customization, though we do allow some changes.

## Custom up arrow filter mode

It can be useful to use a different filter or search mode on the up arrow. For example, you could use ctrl-r for searching globally, but the up arrow for searching history from the current directory only.

Set your config like this:

```
filter_mode_shell_up_key_binding = "directory" # or global, host, directory, etc
```

## Disable up arrow

Our default up-arrow binding can be a bit contentious. Some people love it, some people hate it. Many people who found it a bit jarring at first have since come to love it, so give it a try! Otherwise, if you don't like it, it's easy to disable.

You can also disable either the up-arrow or <kbd>Ctrl-r</kbd> bindings individually, by passing
`--disable-up-arrow` or `--disable-ctrl-r` to the call to `atuin init`:

```
# Bind ctrl-r but not up arrow
eval "$(atuin init zsh --disable-up-arrow)"

# Bind up-arrow but not ctrl-r
eval "$(atuin init zsh --disable-ctrl-r)"
```

If you do not want either key to be bound, either pass both `--disable` arguments, or set the
environment variable `ATUIN_NOBIND` to any value before the call to `atuin init`:

```
## Do not bind any keys
# Either:
eval "$(atuin init zsh --disable-up-arrow --disable-ctrl-r)"

# Or:
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"
```

You can then choose to bind Atuin if needed, do this after the call to init.

## <kbd>Ctrl-n</kbd> key shortcuts

macOS does not have an <kbd>Alt</kbd> key, although terminal emulators can often be configured to map the <kbd>Option</kbd> key to be used as <kbd>Alt</kbd>. *However*, remapping <kbd>Option</kbd> this way may prevent typing some characters, such as using <kbd>Option-3</kbd> to type `#` on the British English layout. For such a scenario, set the `ctrl_n_shortcuts` option to `true` in your config file to replace <kbd>Alt-0</kbd> to <kbd>Alt-9</kbd> shortcuts with <kbd>Ctrl-0</kbd> to <kbd>Ctrl-9</kbd> instead:

```
# Use Ctrl-0 .. Ctrl-9 instead of Alt-0 .. Alt-9 UI shortcuts
ctrl_n_shortcuts = true
```

## zsh

If you'd like to customize your bindings further, it's possible to do so with custom shell config:

Atuin defines the ZLE widget "\_atuin_search_widget"

```
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"

bindkey '^r' _atuin_search_widget

# depends on terminal mode
bindkey '^[[A' _atuin_search_widget
bindkey '^[OA' _atuin_search_widget
```

## bash

```
export ATUIN_NOBIND="true"
eval "$(atuin init bash)"

# bind to ctrl-r, add any other bindings you want here too
bind -x '"\C-r": __atuin_history'
```

## fish

```
set -gx ATUIN_NOBIND "true"
atuin init fish | source

# bind to ctrl-r in normal and insert mode, add any other bindings you want here too
bind \cr _atuin_search
bind -M insert \cr _atuin_search
```

## nu

```
$env.ATUIN_NOBIND = true
atuin init nu | save -f ~/.local/share/atuin/init.nu #make sure you created the directory beforehand with `mkdir ~/.local/share/atuin/init.nu`
source ~/.local/share/atuin/init.nu

#bind to ctrl-r in emacs, vi_normal and vi_insert modes, add any other bindings you want here too
$env.config = (
    $env.config | upsert keybindings (
        $env.config.keybindings
        | append {
            name: atuin
            modifier: control
            keycode: char_r
            mode: [emacs, vi_normal, vi_insert]
            event: { send: executehostcommand cmd: (_atuin_search_cmd) }
        }
    )
)
```


## Atuin UI shortcuts

| Shortcut                                  | Action                                                                        |
| ----------------------------------------- | ----------------------------------------------------------------------------- |
| ctrl + r                                  | Cycle through filter modes                                                    |
| ctrl + s                                  | Cycle through search modes                                                    |
| alt + 1 to alt + 9                        | Select item by the number located near it                                     |
| ctrl + c / ctrl + d / ctrl + g / esc      | Return original                                                               |
| ctrl + ⬅︎ / alt + b                       | Move the cursor to the previous word                                          |
| ctrl + ➡️ / alt + f                       | Move the cursor to the next word                                              |
| ctrl + h / ctrl + b / ⬅︎                  | Move the cursor to the left                                                   |
| ctrl + l / ctrl + f / ➡️                  | Move the cursor to the right                                                  |
| ctrl + a / home                           | Move the cursor to the start of the line                                      |
| ctrl + e / end                            | Move the cursor to the end of the line                                        |
| ctrl + backspace / ctrl + alt + backspace | Remove the previous word / remove the word just before the cursor             |
| ctrl + delete / ctrl + alt + delete       | Remove the next word or the word just after the cursor                        |
| ctrl + w                                  | Remove the word before the cursor even if it spans across the word boundaries |
| ctrl + u                                  | Clear the current line                                                        |
| ctrl + n / ctrl + j / ⬆                  | Select the next item on the list                                              |
| ctrl + p / ctrl + k / ⬇                  | Select the previous item on the list                                          |
| page down                                 | Scroll search results one page down                                           |
| page up                                   | Scroll search results one page up                                             |
| enter                                     | Select highlighted item                                                       |
| ⬇ (with no entry selected)               | Return original or return query depending on settings                         |
| ⬇                                        | Select the next item on the list                                              |

