# Advanced Atuin UI Keybinding

Atuin includes a powerful keybinding system that can be used to fully customize the TUI keyboard shortcuts. Many of the configuration options, like `enter_accept`, `exit_past_line_start`, and `accept_past_line_end`, can be explicitly expressed with this new configuration.

The `[keymap]` section in your config replaces the older `[keys]` section. If any `[keymap]` settings are present, the `[keys]` section is ignored entirely.

!!! warning
    Modifier keys, F1-F24 keys, and some special characters work best - or _only_ work - with a terminal that implements the kitty keyboard protocol. Notably, the default macOS Terminal app does _not_ include this feature. For more information and a list of terminals that are known to support this protocol, see [https://sw.kovidgoyal.net/kitty/keyboard-protocol/](https://sw.kovidgoyal.net/kitty/keyboard-protocol/).

## Keymaps

The Atuin TUI has multiple modes, each with its own keymap. You configure each one under a separate TOML table:

| Config section       | When it is active |
|----------------------|-------------------|
| `[keymap.emacs]`     | Search tab, `keymap_mode = "emacs"` |
| `[keymap.vim-normal]`| Search tab, `keymap_mode = "vim"`, normal mode |
| `[keymap.vim-insert]`| Search tab, `keymap_mode = "vim"`, insert mode |
| `[keymap.inspector]` | Inspector tab (opened with `ctrl-o`) |
| `[keymap.prefix]`    | After pressing the prefix key (`ctrl-a` by default) |

Vim-insert mode inherits all emacs bindings by default, then overrides `esc` and `ctrl-[` to enter normal mode instead of exiting.

You only need to specify the keys you want to change. Unmentioned keys keep their default bindings.

!!! warning
    If you specify a key in your keymap that would normally be changed by an option, like the `enter` key with the `enter_accept` setting, the setting will not take any affect. Those options modify the default keymap based on their setting, but if you override the key in the keymap, you're responsible for managing correct behavior.

## Key format

Keys are specified as TOML string keys using a human-readable format.

### Basic keys

Lowercase letters, digits, and named keys:

```
"a", "z", "1", "9"
"enter", "esc", "tab", "space", "backspace", "delete"
"up", "down", "left", "right"
"home", "end", "pageup", "pagedown"
"f1", "f2", ... "f12", ... "f24"
```

`return` is an alias for `enter`. `escape` is an alias for `esc`. `del` is an alias for `delete`.

!!! warning "macOS delete key"
    The key labeled "delete" on Mac keyboards sends `backspace` (it deletes the character *before* the cursor). The `delete` key in Atuin refers to forward-delete, which is `fn+delete` on a Mac keyboard.

### Modifiers

Modifiers are prefixed with a dash separator. Multiple modifiers can be combined:

```
"ctrl-c", "alt-f", "ctrl-alt-x"
```

Available modifiers: `ctrl`, `alt`, `shift`, `super` (also accepted as `cmd` or `win`).

!!! warning
    The `super` modifier (Cmd on macOS, Win on Windows) **requires** the kitty keyboard protocol. Only terminals that implement this protocol will report the Super modifier to applications. Even in supported terminals, some Super+key combinations may be intercepted by the terminal or OS (e.g. Cmd+C for copy, Cmd+V for paste, or Cmd+T for opening a new tab).

### Uppercase letters

An uppercase letter represents itself without needing a `shift` modifier. For example, `"G"` matches the `shift+g` key press.

### Special characters

Some special characters are written out directly:

```
"?", "/", "[", "]", "$"
```

### Shifted and punctuation keys

When you press a key like `Shift+1`, your terminal sends the resulting character (`!`) rather than "shift-1". To bind shifted punctuation keys, use the character directly:

```toml
[keymap.emacs]
"!" = "some-action"    # Binds to Shift+1
"@" = "some-action"    # Binds to Shift+2
"#" = "some-action"    # Binds to Shift+3
"$" = "cursor-end"     # Binds to Shift+4 (vim $ motion)
```

Any single character can be used as a key binding.

!!! note
    The `shift` modifier is still valid for non-character keys like `"shift-tab"` or `"shift-up"`.

### Media keys

Media keys are supported on terminals that implement the kitty keyboard protocol with `DISAMBIGUATE_ESCAPE_CODES` enabled:

```
"play", "pause", "playpause", "stop"
"fastforward", "rewind", "tracknext", "trackprevious"
"record", "lowervolume", "raisevolume", "mutevolume", "mute"
```

### Multi-key sequences

Separate keys with a space to define a sequence. The first key is buffered until the second key arrives:

```
"g g"
```

If the second key does not complete a known sequence, both keys are handled individually.

## Keymap format

Each entry in a keymap section maps a key to either a simple action or a conditional rule list.

### Simple binding

Maps a key directly to a single action, with no conditions:

```toml
[keymap.emacs]
"ctrl-c" = "return-original"
"enter" = "accept"
```

### Conditional binding

Maps a key to an ordered list of rules. Each rule has an `action` and an optional `when` condition. Rules are evaluated top-to-bottom; the first rule whose condition matches (or that has no condition) wins.

```toml
[keymap.emacs]
"left" = [
  { when = "cursor-at-start", action = "exit" },
  { action = "cursor-left" },
]
```

In this example, pressing left when the cursor is at position 0 exits the TUI. Otherwise, it moves the cursor left.

A rule without a `when` field is unconditional and always matches. It is typically placed last as a fallback.

!!! warning "Override semantics"
    When you specify a key in `[keymap]`, it **replaces** the **entire** default binding for that key. Other keys you don't mention keep their defaults.

## Actions

Actions are specified as kebab-case strings.

### Cursor movement

| Action | Description |
|--------|-------------|
| `cursor-left` | Move cursor one character left |
| `cursor-right` | Move cursor one character right |
| `cursor-word-left` | Move cursor one word left |
| `cursor-word-right` | Move cursor one word right |
| `cursor-word-end` | Move cursor to end of current/next word (vim `e` motion) |
| `cursor-start` | Move cursor to start of line |
| `cursor-end` | Move cursor to end of line |

### Editing

| Action | Description |
|--------|-------------|
| `delete-char-before` | Delete the character before the cursor (backspace) |
| `delete-char-after` | Delete the character after the cursor (delete) |
| `delete-word-before` | Delete the word before the cursor |
| `delete-word-after` | Delete the word after the cursor |
| `delete-to-word-boundary` | Delete to the next word boundary (like `ctrl-w`) |
| `clear-line` | Clear the entire input line |
| `clear-to-start` | Clear the start of input line |
| `clear-to-end` | Clear the end of input line |

### List navigation

| Action | Description |
|--------|-------------|
| `select-next` | Move selection to the next item in the results list |
| `select-previous` | Move selection to the previous item in the results list |
| `scroll-half-page-up` | Scroll half a page up |
| `scroll-half-page-down` | Scroll half a page down |
| `scroll-page-up` | Scroll a full page up |
| `scroll-page-down` | Scroll a full page down |
| `scroll-to-top` | Jump to the top of the list |
| `scroll-to-bottom` | Jump to the bottom of the list |
| `scroll-to-screen-top` | Jump to the top of the visible screen |
| `scroll-to-screen-middle` | Jump to the middle of the visible screen |
| `scroll-to-screen-bottom` | Jump to the bottom of the visible screen |

Note: `select-next` and `select-previous` respect the `invert` setting. When `invert` is true, the visual direction is flipped.

### Commands

| Action | Description |
|--------|-------------|
| `accept` | Accept the selected entry and **execute it immediately** |
| `accept-N` | Accept the Nth entry below the selection and execute it (e.g. `accept-1` through `accept-9`) |
| `return-selection` | Return the selected entry to the command line **without executing** |
| `return-selection-N` | Return the Nth entry below the selection without executing (e.g. `return-selection-1` through `return-selection-9`) |
| `return-original` | Close the TUI and return the original command line text |
| `return-query` | Close the TUI and return the current search query |
| `copy` | Copy the selected entry to the clipboard |
| `delete` | Delete the selected entry from history |
| `exit` | Exit the TUI (behavior depends on the `exit_mode` setting) |
| `redraw` | Redraw the screen |
| `cycle-filter-mode` | Cycle through filter modes (global, host, session, directory) |
| `cycle-search-mode` | Cycle through search modes (fuzzy, prefix, fulltext, skim) |
| `toggle-tab` | Toggle between the search tab and inspector tab |
| `switch-context` | Switch to the [context](../guide/advanced-usage.md#context-switch) of the currently selected command |
| `clear-context` | Return to the initial [context](../guide/advanced-usage.md#context-switch) |

The difference between `accept` and `return-selection`: `accept` runs the command immediately when the TUI closes, while `return-selection` places it on your command line for further editing before you press enter. The `enter_accept` setting controls which of these the default `enter` key uses.

### Mode changes

| Action | Description |
|--------|-------------|
| `vim-enter-normal` | Switch to vim normal mode |
| `vim-enter-insert` | Switch to vim insert mode (cursor stays in place) |
| `vim-enter-insert-after` | Switch to vim insert mode (cursor moves right, like vim `a`) |
| `vim-enter-insert-at-start` | Move to start of line and enter vim insert mode (like vim `I`) |
| `vim-enter-insert-at-end` | Move to end of line and enter vim insert mode (like vim `A`) |
| `vim-search-insert` | Clear the search input and enter vim insert mode (like vim `?` or `/`) |
| `vim-change-to-end` | Delete to end of line and enter vim insert mode (like vim `C`) |
| `enter-prefix-mode` | Enter prefix mode (waits for one more key, e.g. `d` for delete) |

### Inspector

| Action | Description |
|--------|-------------|
| `inspect-previous` | Inspect the previous entry (in the inspector tab) |
| `inspect-next` | Inspect the next entry (in the inspector tab) |

### Special

| Action | Description |
|--------|-------------|
| `noop` | Do nothing (useful for disabling a default binding) |

## Conditions

Conditions let a single key do different things depending on the current state. They are specified as strings in the `when` field of a rule.

### Condition atoms

| Condition | True when |
|-----------|-----------|
| `cursor-at-start` | The cursor is at position 0 |
| `cursor-at-end` | The cursor is at the end of the input |
| `input-empty` | The input line is empty (no text entered) |
| `original-input-empty` | The original query passed to the TUI was empty |
| `list-at-start` | The selection is at the first entry (index 0) |
| `list-at-end` | The selection is at the last entry |
| `no-results` | The search returned zero results |
| `has-results` | The search returned at least one result |
| `has-context` | The context comes from a previously selected command (`switch-context`) |

### Boolean expressions

Conditions support boolean operators with standard precedence (`!` binds tightest, then `&&`, then `||`). Parentheses can override precedence.

```toml
# Negation
{ when = "!no-results", action = "select-next" }

# Conjunction (AND)
{ when = "cursor-at-start && input-empty", action = "exit" }

# Disjunction (OR)
{ when = "list-at-start || no-results", action = "exit" }

# Grouping with parentheses
{ when = "(cursor-at-start && !input-empty) || no-results", action = "return-original" }
```

## Examples

### Reproducing the default `[keys]` behaviors

The default keymaps already encode the standard `[keys]` behaviors. Here is what they look like as explicit `[keymap]` entries for reference.

**`scroll_exits = true`** (default) -- exit when scrolling past the first entry:

```toml
[keymap.emacs]
"down" = [
  { when = "list-at-start", action = "exit" },
  { action = "select-next" },
]
```

**`exit_past_line_start = true`** (default) -- exit when pressing left at position 0:

```toml
[keymap.emacs]
"left" = [
  { when = "cursor-at-start", action = "exit" },
  { action = "cursor-left" },
]
```

**`accept_past_line_end = true`** (default) -- accept when pressing right at the end:

```toml
[keymap.emacs]
"right" = [
  { when = "cursor-at-end", action = "accept" },
  { action = "cursor-right" },
]
```

**`accept_past_line_start = true`** -- accept when pressing left at position 0 (off by default):

```toml
[keymap.emacs]
"left" = [
  { when = "cursor-at-start", action = "accept" },
  { action = "cursor-left" },
]
```

**`accept_with_backspace = true`** -- accept when pressing backspace with empty input (off by default):

```toml
[keymap.emacs]
"backspace" = [
  { when = "cursor-at-start", action = "accept" },
  { action = "delete-char-before" },
]
```

### Disabling scroll-exit

To make `down` always scroll without ever exiting:

```toml
[keymap.emacs]
"down" = "select-next"
```

### Disabling a key entirely

Use `noop` to make a key do nothing:

```toml
[keymap.emacs]
"ctrl-d" = "noop"
```

### ctrl-d to exit only when input is empty

```toml
[keymap.emacs]
"ctrl-d" = [
  { when = "input-empty", action = "exit" },
  { action = "delete-char-after" },
]
```

### Making enter return the selection without executing

```toml
[keymap.emacs]
"enter" = "return-selection"
```

This is equivalent to setting `enter_accept = false`, but expressed directly as a keybinding.

### Custom vim-normal bindings

```toml
[keymap.vim-normal]
# Use 'q' to quit
"q" = "exit"

# Use 'x' to delete the selected entry
"x" = "delete"

# Use 'y' to copy
"y" = "copy"
```

### Custom inspector bindings

```toml
[keymap.inspector]
# Use 'delete' key in inspector to remove entries
"delete" = "delete"
```

## Relationship with `[keys]`

The `[keymap]` section is a more powerful replacement for the `[keys]` section. The two are **mutually exclusive**:

- If you have any `[keymap]` settings, the entire `[keys]` section is ignored. Defaults are built from the standard `[keys]` values, and then your `[keymap]` overrides are applied on top.
- If you have no `[keymap]` settings, the `[keys]` section works as before for backward compatibility.

If you are migrating from `[keys]` to `[keymap]`, here is how the old flags map:

| `[keys]` setting | Equivalent `[keymap]` |
|------------------|-----------------------|
| `scroll_exits = false` | `"down" = "select-next"` and `"up" = "select-previous"` in the relevant keymap |
| `exit_past_line_start = false` | `"left" = "cursor-left"` |
| `accept_past_line_end = false` | `"right" = "cursor-right"` |
| `accept_past_line_start = true` | `"left" = [{ when = "cursor-at-start", action = "accept" }, { action = "cursor-left" }]` |
| `accept_with_backspace = true` | `"backspace" = [{ when = "cursor-at-start", action = "accept" }, { action = "delete-char-before" }]` |
| `prefix = "x"` | Prefix key becomes `ctrl-x` (set in the emacs/vim keymaps) |
