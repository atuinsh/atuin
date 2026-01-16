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

* default theme (can be explicitly referenced with an empty name `""`)
* `autumn` theme
* `marine` theme

These are present to ensure users and developers can try out theming, but in general, you
will need to download themes or make your own.

If you are writing your own themes, you can add the following line to get additional output:

```
debug = true
```

to the same config block. This will print out any color names that cannot be parsed from
the requested theme.

A final optional setting is available:

```
max_depth: 10
```

which sets the maximum levels of theme parents to traverse. This should not need to be
explicitly added in normal use.

## Usage

### Theme structure

Themes are maps from *Meanings*, describing the developer's intentions,
to (at present) colors. In future, this may be expanded to allow richer style support.

*Meanings* are from an enum with the following values:

* `AlertInfo`: alerting the user at an INFO level
* `AlertWarn`: alerting the user at a WARN level
* `AlertError`: alerting the user at an ERROR level
* `Annotation`: less-critical, supporting text
* `Base`: default foreground color
* `Guidance`: instructing the user as help or context
* `Important`: drawing the user's attention to information
* `Title`: titling a section or view

These may expand over time as they are added to Atuin's codebase, but Atuin
should have fallbacks added for any new *Meanings* so that, whether themes limit to
the present list or take advantage of new *Meanings* in future, they should
keep working sensibly.

**Note for Atuin contributors**: please do identify and, where appropriate during your own
PRs, extend the Meanings enum if needed (along with a fallback Meaning!).

### Theme creation

When a theme name is read but not yet loaded, Atuin will look for it in the folder
`~/.config/atuin/themes/` unless overridden by the `ATUIN_THEME_DIR` environment
variable. It will attempt to open a file of name `THEMENAME.toml` and read it as a
map from *Meanings* to colors.

Colors may be specified either as names from the [palette](https://ogeon.github.io/docs/palette/master/palette/named/index.html)
crate in lowercase, or as six-character hex codes, prefixed with `#`. For example,
the following are valid color names:

* `#ff0088`
* `teal`
* `powderblue`

A theme file, say `my-theme.toml` can then be built up, such as:

```toml
[theme]
name = "my-theme"
parent = "autumn"

[colors]
AlertInfo = "green"
Guidance = "#888844"

```

where not all of the *Meanings* need to be explicitly defined. If they are absent,
then the color will be chosen from the parent theme, if one is defined, or if that
key is missing in the `theme` block, from the default theme.

This theme file should be moved to `~/.config/atuin/themes/my-theme.toml` and the
following added to `~/.config/atuin/config.toml`:

```
[theme]
name = "my-theme"
```

When you next run Atuin, your theme should be applied.
