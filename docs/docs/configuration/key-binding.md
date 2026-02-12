# Key Binding

## Custom up arrow filter mode

It can be useful to use a different filter or search mode on the up arrow. For example, you could use ctrl-r for searching globally, but the up arrow for searching history from the current directory only.

Set your config like this:

```
filter_mode_shell_up_key_binding = "directory" # or global, host, directory, etc
```

## Disable up arrow

Our default up-arrow binding can be a bit contentious. Some people love it, some people hate it. Many people who found it a bit jarring at first have since come to love it, so give it a try!

It becomes much more powerful if you consider binding a different filter mode to the up arrow. For example, on "up" Atuin can default to searching all history for the current directory only, while ctrl-r searches history globally. See the [config](config.md#filter_mode_shell_up_key_binding) for more.

Otherwise, if you don't like it, it's easy to disable.

You can also disable either the up-arrow or ++ctrl+r++ bindings individually, by passing
`--disable-up-arrow` or `--disable-ctrl-r` to the call to `atuin init` in your shell config file:

An example for zsh:
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

## Enter key behavior

By default, the `enter` key will directly execute the selected command instead of letting you edit it like the `tab` key. If you want to change this behavior, set `enter_accept = false` in your config. For more details: [enter_accept](config.md#enter_accept).

## Ctrl-n key shortcuts

macOS does not have an ++alt++ key, although terminal emulators can often be configured to map the ++option++ key to be used as ++alt++. *However*, remapping ++option++ this way may prevent typing some characters, such as using ++option+3++ to type `#` on the British English layout. For such a scenario, set the `ctrl_n_shortcuts` option to `true` in your config file to replace ++alt+0++ to ++alt+9++ shortcuts with ++ctrl+0++ to ++ctrl+9++ instead:

```
# Use Ctrl-0 .. Ctrl-9 instead of Alt-0 .. Alt-9 UI shortcuts
ctrl_n_shortcuts = true
```

Ghostty on Linux maps ++alt+1++ .. ++alt+9++ for switching between tabs by number. To disable this behavior either add the following to ~/.config/ghostty/config:
```
keybind=alt+one=unbind
keybind=alt+two=unbind
keybind=alt+three=unbind
keybind=alt+four=unbind
keybind=alt+five=unbind
keybind=alt+six=unbind
keybind=alt+seven=unbind
keybind=alt+eight=unbind
keybind=alt+nine=unbind
```
(this will disable tab switching by ++alt+n++)
or use the `ctrl_n_shortcuts` as outlined above.

## zsh

If you'd like to customize your bindings further, it's possible to do so with custom shell config:

Atuin defines the ZLE widgets "atuin-search" and "atuin-up-search".  The latter
can be used for the keybindings to the ++up++ key and similar keys.

Note: instead use the widget names "\_atuin\_search\_widget" and "\_atuin\_up\_search\_widget", respectively, in `atuin < 18.0`

```
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"

bindkey '^r' atuin-search

# bind to the up key, which depends on terminal mode
bindkey '^[[A' atuin-up-search
bindkey '^[OA' atuin-up-search
```

For the keybindings in vi mode, "atuin-search-viins", "atuin-search-vicmd",
"atuin-up-search-viins", and "atuin-up-search-vicmd" (`atuin >= 18.0`) can be
used in combination with the config
["keymap\_mode"](config.md#keymap_mode)
(`atuin >= 18.0`) to start the Atuin search in respective keymap modes.

## bash

Atuin (`>= 18.10.0`) provides a shell function `atuin-bind` to set up
keybindings easily:

```
atuin-bind [-m KEYMAP] KEYSEQ COMMAND
```

`KEYMAP` is one of `emacs`, `vi-insert`, and `vi-command` and specifies the
target keymap where the keybinding is defined.  `KEYSEQ` specifies a key
sequence in the format used in `bind '"KEYSEQ": ...'`.  `COMMAND` specifies a
shell command to run with the keybindings.  The following special commands can
be used as well as an arbitrary shell command:

| Command                 | Description                                                                         |
| ----------------------- | ----------------------------------------------------------------------------------- |
| `atuin-search`          | Standard search                                                                     |
| `atuin-search-emacs`    | Standard search with the `emacs` keymap mode                                        |
| `atuin-search-viins`    | Standard search with the `vim-insert` keymap mode                                   |
| `atuin-search-vicmd`    | Standard search with the `vim-normal` keymap mode                                   |
| `atuin-up-search`       | Search command for <kbd>up</kbd> or similar keys                                    |
| `atuin-up-search-emacs` | Search command for <kbd>up</kbd> or similar keys, with the `emacs` keymap mode      |
| `atuin-up-search-viins` | Search command for <kbd>up</kbd> or similar keys, with the `vim-insert` keymap mode |
| `atuin-up-search-vicmd` | Search command for <kbd>up</kbd> or similar keys, with the `vim-nomarl` keymap mode |

The keymap mode controls the initial keymap in the Atuin search and is
determined in combination with the config
["keymap\_mode"](config.md#keymap_mode)
(`atuin >= 18.0`).


```
export ATUIN_NOBIND="true"
eval "$(atuin init bash)"

# bind to ctrl-r, add any other bindings you want here too
atuin-bind '\C-r' atuin-search
# example of CTRL-upkey
# atuin-bind '\e[1;5A' atuin-search

# bind to the up key, which depends on terminal mode
atuin-bind '\e[A' atuin-up-search
atuin-bind '\eOA' atuin-up-search
```

With older versions of Atuin, the user needs to bind a bindable shell function
"`__atuin_history`" directly using Bash's `bind`.  The flag
`--shell-up-key-binding` can be optionally specified to the first argument for
keybindings to the <kbd>up</kbd> key or similar keys.  For the keybindings in
the `vi` editing mode, the options `--keymap-mode=vim-insert` and the keymap
mode `--keymap-mode=vim-normal` (`atuin >= 18.0`) can be additionally specified
to the shell function `__atuin_history`.

## fish
Edit key bindings in FISH shell by adding the following to ~/.config/fish/config.fish

```
set -gx ATUIN_NOBIND "true"
atuin init fish | source

# bind to ctrl-r in normal and insert mode, add any other bindings you want here too
bind \cr _atuin_search
bind -M insert \cr _atuin_search
```

For the ++up++ keybinding, `_atuin_bind_up` can be used instead of `_atuin_search`.

Adding the useful alternative key binding of ++ctrl+up++ is tricky and determined by the terminals adherence to terminfo(5).

Conveniently FISH uses a command to capture keystrokes and advises you of the exact command to add for your specific terminal.
In your terminal, run `fish_key_reader` then punch the desired keystroke/s.

For example, in Gnome Terminal the output to ++ctrl+up++ is `bind \e\[1\;5A 'do something'`

So, adding this to the above sample, `bind \e\[1\;5A _atuin_search` will provide the additional search keybinding.

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
|-------------------------------------------|-------------------------------------------------------------------------------|
| enter                                     | Execute selected item                                                         |
| tab                                       | Select item and edit                                                          |
| ctrl + r                                  | Cycle through filter modes                                                    |
| ctrl + s                                  | Cycle through search modes                                                    |
| alt + 1 to alt + 9                        | Select item by the number located near it                                     |
| ctrl + c / ctrl + d / ctrl + g / esc      | Return original                                                               |
| ctrl + y                                  | Copy selected item to clipboard                                               |
| ctrl + ← / alt + b                        | Move the cursor to the previous word                                          |
| ctrl + → / alt + f                        | Move the cursor to the next word                                              |
| ctrl + b / ←                              | Move the cursor to the left                                                   |
| ctrl + f / →                              | Move the cursor to the right                                                  |
| ctrl + a / home                           | Move the cursor to the start of the line                                      |
| ctrl + e / end                            | Move the cursor to the end of the line                                        |
| ctrl + backspace / ctrl + alt + backspace | Remove the previous word / remove the word just before the cursor             |
| ctrl + delete / ctrl + alt + delete       | Remove the next word or the word just after the cursor                        |
| ctrl + w                                  | Remove the word before the cursor even if it spans across the word boundaries |
| ctrl + u                                  | Clear the current line                                                        |
| ctrl + n / ctrl + j / ↑                   | Select the next item on the list                                              |
| ctrl + p / ctrl + k / ↓                   | Select the previous item on the list                                          |
| ctrl + o                                  | Open the [inspector](#inspector)                                              |
| page down                                 | Scroll search results one page down                                           |
| page up                                   | Scroll search results one page up                                             |
| ↓ (with no entry selected)                | Return original or return query depending on [settings](config.md#exit_mode)  |
| ↓                                         | Select the next item on the list                                              |
| ctrl + a, c                               | Switch to the context of the currently selected command / return to default   |


### Vim mode
If [vim is enabled in the config](config.md#keymap_mode), the following keybindings are enabled:

| Shortcut | Mode   | Action                                     |
| -------- | ------ | ------------------------------------------ |
| k        | Normal | Selects the next item on the list          |
| j        | Normal | Selects the previous item on the list      |
| h        | Normal | Move cursor left                           |
| l        | Normal | Move cursor right                          |
| 0        | Normal | Move cursor to start of line               |
| $        | Normal | Move cursor to end of line                 |
| w        | Normal | Move cursor to next word                   |
| b        | Normal | Move cursor to previous word               |
| e        | Normal | Move cursor to end of current/next word    |
| x        | Normal | Delete character under cursor              |
| dd       | Normal | Clear the entire line                      |
| D        | Normal | Delete to end of line                      |
| C        | Normal | Delete to end of line and enter insert     |
| i        | Normal | Enters insert mode                         |
| I        | Normal | Move to start of line and enter insert     |
| a        | Normal | Move right and enter insert mode           |
| A        | Normal | Move to end of line and enter insert       |
| Ctrl+u   | Normal | Half-page up (toward visual top)           |
| Ctrl+d   | Normal | Half-page down (toward visual bottom)      |
| Ctrl+b   | Normal | Full-page up (toward visual top)           |
| Ctrl+f   | Normal | Full-page down (toward visual bottom)      |
| G        | Normal | Jump to visual bottom of history           |
| gg       | Normal | Jump to visual top of history              |
| H        | Normal | Jump to top of visible screen              |
| M        | Normal | Jump to middle of visible screen           |
| L        | Normal | Jump to bottom of visible screen           |
| ? or /   | Normal | Clear input and enter insert mode          |
| 1-9      | Normal | Select item by number                      |
| enter    | Normal | Execute selected item (respects enter_accept) |
| Esc      | Insert | Enters normal mode                         |


### Inspector
Open the inspector with ctrl + o

| Shortcut  | Action                                        |
| --------- | --------------------------------------------- |
| Esc       | Close the inspector, returning to the shell   |
| ctrl + o  | Close the inspector, returning to search view |
| ctrl + d  | Delete the inspected item from the history    |
| ↑         | Inspect the previous item in the history      |
| ↓         | Inspect the next item in the history          |
| page up   | Inspect the previous item in the history      |
| page down | Inspect the next item in the history          |
| j / k     | Navigate items (when vim mode is enabled)     |
| enter     | Execute selected item (respects enter_accept) |
| tab       | Select current item and edit                  |
