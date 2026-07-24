# Theming

Available in Atuin >= 18.4

For terminal interface customization, Atuin supports user and built-in color themes.

Atuin ships with only a couple of built-in alternative themes, but more can be added via TOML files.

## Required config

The following is required in your config file (`~/.config/atuin/config.toml`)

```
[theme]
name = "THEMENAME"
```

Where `THEMENAME` is a known theme. The following themes are available out-of-the-box:

- `default` theme
- `autumn` theme
- `marine` theme
- `(none)` theme (removes all styling)

These are present to make sure users and developers can try out theming, but in general, you will need to download themes or make your own.

If you're writing your own themes, you can add the following line to get additional output:

```
debug = true
```

to the same config block. This will print out any color names that can't be parsed from the requested theme.

A final optional setting is available:

```
max_depth = 10
```

which sets the maximum levels of theme parents to traverse. This shouldn't need to be explicitly added in normal use.

## Usage

### Theme structure

Themes are maps from *Meanings*, describing the developer's intentions, to (at present) colors. In future, this may be expanded to allow richer style support.

*Meanings* are from an enum with the following values:

- `AlertInfo`: alerting the user at an INFO level
- `AlertWarn`: alerting the user at a WARN level
- `AlertError`: alerting the user at an ERROR level
- `Annotation`: less-critical, supporting text
- `Base`: default foreground color
- `Guidance`: instructing the user as help or context
- `Important`: drawing the user's attention to information
- `Title`: titling a section or view
- `Muted`: anodyne, usually grey, foreground for contrast with other colors. Normally equivalent to the base color, but themes can change the base color, with less risk of breaking intentional color contrasts (for example, stacked bar charts)
- `SyntaxCommand`: the command word when syntax highlighting shell commands (`git` in `git status`)
- `SyntaxFlag`: a `-f`/`--flag` argument
- `SyntaxString`: a quoted string
- `SyntaxVariable`: `$VAR`, `${VAR}` or a `FOO=bar` assignment
- `SyntaxOperator`: operators such as `|`, `&&`, `;`, `>`
- `SyntaxComment`: a `# comment`

These may expand over time as they're added to Atuin's codebase. Atuin should have fallbacks for any new *Meanings*, so themes keep working sensibly whether they limit themselves to the present list or take advantage of new *Meanings* later.

**Note for Atuin contributors**: please do identify and, where appropriate during your own PRs, extend the Meanings enum if needed (along with a fallback Meaning!).

### Theme creation

When a theme name is read but not yet loaded, Atuin will look for it in the folder `~/.config/atuin/themes/` unless overridden by the `ATUIN_THEME_DIR` environment variable. It will attempt to open a file of name `THEMENAME.toml` and read it as a map from *Meanings* to foreground colors.

Note that, at present, it's not possible to specify the default terminal color explicitly in a theme file. However, the default theme Base color will always be unset and therefore will be the user's default terminal color. Hence, you should only override the Base color in your theme, or derive from a theme that does so, if your theme wouldn't make sense otherwise. For example, the `marine` theme is intended to make everything green/blue, so it overrides the Base color, but the `autumn` theme only seeks to make the custom colors warmer, so it doesn't.

Colors may be specified either as names from the [palette](https://ogeon.github.io/docs/palette/master/palette/named/index.html) crate in lowercase, or as six-character hex codes, prefixed with `#`. To explicitly select ANSI colors by integer, or for more flexibility in general, prefix the color with `@`. Crossterm handles the rest of the string with its own color parsing. For examples, see [crossterm's color deserialization tests](https://github.com/crossterm-rs/crossterm/blob/5d50d8da62c5e034ef8b2787a771a2c0f9b3b2f9/src/style/types/color.rs#L389), remembering the need to add a `@` prefix for Atuin.

For example, the following are valid color names:

- `#ff0088`
- `teal`
- `powderblue`
- `@ansi_(255)`
- `@rgb_(255, 128, 0)`

You can also express colors through Crossterm-supported strings, prefixed by `@`. For example,

- `@ansi_(123)`
- `@dark_yellow`

While there isn't currently an official reference, you can see examples in the [crossterm tests](https://docs.rs/crossterm/latest/src/crossterm/style/types/color.rs.html#376). As this is passed straight to Crossterm, using [ANSI codes](https://www.ditig.com/256-colors-cheat-sheet) can be helpful for ensuring your theme is compatible with 256-color terminals.

A theme file, say `my-theme.toml` can then be built up, such as:

```
[theme]
name = "my-theme"
parent = "autumn"

[colors]
AlertInfo = "green"
Guidance = "#888844"
```

where not all of the *Meanings* need to be explicitly defined. If they're absent, then the color will be chosen from the parent theme, if one is defined, or if that key is missing in the `theme` block, from the `default` theme.

If the named theme is missing entirely, that's an error. The theme then drops to `(none)` and leaves Atuin unstyled, rather than falling back to the default or any other theme.

This theme file should be moved to `~/.config/atuin/themes/my-theme.toml` and the following added to `~/.config/atuin/config.toml`:

```
[theme]
name = "my-theme"
```

When you next run Atuin, your theme should be applied.
