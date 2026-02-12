# Basic Usage

Now that you're all set up and running, here's a quick walkthrough of how you can use Atuin best.

## What does Atuin record?

While you work, Atuin records:

1. The command you run
2. The directory you ran it in
3. The time you ran it, and how long it took to run
4. The exit code of the command
5. The hostname + user of the machine
6. The shell session you ran it in

## Opening and using the TUI

At any time, you can open the TUI with the default keybindings of the up arrow, or ctrl-r.

Once in the TUI, press enter to immediately execute a command, or press tab to insert it into your shell for editing.

While searching in the TUI, you can adjust the "filter mode" by repeatedly pressing ctrl-r. Atuin can filter by:

1. All hosts
2. Just your local machine
3. The current directory only
4. The current shell session only

See the [advanced usage](advanced-usage.md) page for more options.

## Common config adjustment

For a full set of config values, please see the [config reference page](../configuration/config.md).

The default configuration file is located at `~/.config/atuin/config.toml`.

### Keybindings

We have a [full page dedicated to keybinding adjustments](../configuration/key-binding.md).
There are a whole bunch of options there, including disabling the up arrow behavior if you don't like it.

### Enter to run

You may prefer that Atuin always inserts the selected command for editing. To configure this, set

```
enter_accept = false
```

in your config file.

### Inline window

If you find the full screen TUI overwhelming or too large, you can adjust it like so:

```
# height of the search window
inline_height = 40
```

You may also prefer the compact UI mode:

```
style = "compact"
```
