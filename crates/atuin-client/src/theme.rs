use std::collections::HashMap;
use std::error;
use std::sync::LazyLock;

use atuin_common::path::PathBufExt as _;
use atuin_common::userio;
use config::{Config, File as ConfigFile, FileFormat};
use palette::named;
use serde::{Deserialize, Serialize};

// Collection of settable "meanings" that can have colors set.
// NOTE: You can add a new meaning here without breaking backwards compatibility but please:
//     - update the atuin/docs repository, which has a list of available meanings
//     - add a fallback in the MEANING_FALLBACKS below, so that themes which do not have it
//       get a sensible fallback (see Title as an example)
#[derive(
    Serialize, Deserialize, Copy, Clone, Hash, Debug, Eq, PartialEq, strum_macros::Display,
)]
#[strum(serialize_all = "camel_case")]
pub enum Meaning {
    AlertInfo,
    AlertWarn,
    AlertError,
    Annotation,
    Base,
    Guidance,
    Important,
    Title,
    Muted,
    SyntaxCommand,
    SyntaxFlag,
    SyntaxString,
    SyntaxVariable,
    SyntaxOperator,
    SyntaxComment,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ThemeConfig {
    // Definition of the theme
    pub theme: ThemeDefinitionConfigBlock,

    // Colors
    pub colors: HashMap<Meaning, String>,
}

impl ThemeConfig {
    fn parse(config: Config) -> Result<Self, Box<dyn error::Error>> {
        config.try_deserialize().map_err(|e| format!("Failed to deserialize theme: {e}").into())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ThemeDefinitionConfigBlock {
    /// Name of theme ("default" for base)
    pub name: String,

    /// Whether any theme should be treated as a parent _if available_
    pub parent: Option<String>,
}

use crossterm::style::{Attribute, Attributes, Color, ContentStyle};

// For now, a theme is loaded as a mapping of meanings to colors, but it may be desirable to
// expand that in the future to general styles, so we populate a Meaning->ContentStyle hashmap.
pub struct Theme {
    pub name: String,
    pub parent: Option<String>,
    pub styles: HashMap<Meaning, ContentStyle>,
}

// Themes have a number of convenience functions for the most commonly used meanings.
// The general purpose `as_style` routine gives back a style, but for ease-of-use and to keep
// theme-related boilerplate minimal, the convenience functions give a color.
impl Theme {
    // This is the base "default" color, for general text
    #[must_use]
    pub fn get_base(&self) -> ContentStyle {
        self.styles[&Meaning::Base]
    }

    #[must_use]
    pub fn get_info(&self) -> ContentStyle {
        self.get_alert(tracing::Level::INFO)
    }

    #[must_use]
    pub fn get_warning(&self) -> ContentStyle {
        self.get_alert(tracing::Level::WARN)
    }

    #[must_use]
    pub fn get_error(&self) -> ContentStyle {
        self.get_alert(tracing::Level::ERROR)
    }

    // The alert meanings may be chosen by the Level enum, rather than the methods above
    // or the full Meaning enum, to simplify programmatic selection of a log-level.
    pub fn get_alert(&self, severity: tracing::Level) -> ContentStyle {
        self.styles[ALERT_TYPES.get(&severity).unwrap()]
    }

    #[must_use]
    pub fn new(
        name: String,
        parent: Option<String>,
        styles: HashMap<Meaning, ContentStyle>,
    ) -> Self {
        Self {
            name,
            parent,
            styles,
        }
    }

    pub fn closest_meaning<'a>(&self, meaning: &'a Meaning) -> &'a Meaning {
        if self.styles.contains_key(meaning) {
            meaning
        } else if MEANING_FALLBACKS.contains_key(meaning) {
            self.closest_meaning(&MEANING_FALLBACKS[meaning])
        } else {
            &Meaning::Base
        }
    }

    // General access - if you have a meaning, this will give you a (crossterm) style
    #[must_use]
    pub fn as_style(&self, meaning: Meaning) -> ContentStyle {
        self.styles[self.closest_meaning(&meaning)]
    }

    /// Builds a theme from color names over the styles of `parent`.
    ///
    /// A color name that does not parse is logged and left unstyled, so that one bad color does
    /// not stop the theme from loading.
    #[must_use]
    pub fn from_foreground_colors(
        name: String,
        parent: Option<&Self>,
        foreground_colors: &HashMap<Meaning, String>,
    ) -> Self {
        let styles: HashMap<Meaning, ContentStyle> = foreground_colors
            .iter()
            .map(|(name, color)| {
                (
                    *name,
                    StyleFactory::from_fg_string(color).unwrap_or_else(|err| {
                        userio::log::warn!(
                            "theme-color-invalid",
                            name = name.to_string(),
                            color = color,
                            error = err
                        );
                        ContentStyle::default()
                    }),
                )
            })
            .collect();
        Self::from_map(name, parent, &styles)
    }

    // Boil down a meaning-color hashmap into a theme, by taking the defaults
    // for any unknown colors
    fn from_map(
        name: String,
        parent: Option<&Self>,
        overrides: &HashMap<Meaning, ContentStyle>,
    ) -> Self {
        let styles = match parent {
            Some(theme) => Box::new(theme.styles.clone()),
            None => Box::new(DEFAULT_THEME.styles.clone()),
        }
        .iter()
        .map(|(name, color)| match overrides.get(name) {
            Some(value) => (*name, *value),
            None => (*name, *color),
        })
        .collect();
        Self::new(name, parent.map(|p| p.name.clone()), styles)
    }
}

// Use palette to get a color from a string name, if possible
fn from_string(name: &str) -> Result<Color, String> {
    if name.is_empty() {
        return Err("Empty string".into());
    }
    let first_char = name.chars().next().unwrap();
    match first_char {
        '#' => {
            let hexcode = &name[1..];
            let vec: Vec<u8> = hexcode
                .chars()
                .collect::<Vec<char>>()
                .chunks(2)
                .map(|pair| u8::from_str_radix(pair.iter().collect::<String>().as_str(), 16))
                .filter_map(|n| n.ok())
                .collect();
            if vec.len() != 3 {
                return Err("Could not parse 3 hex values from string".into());
            }
            Ok(Color::Rgb {
                r: vec[0],
                g: vec[1],
                b: vec[2],
            })
        }
        '@' => {
            // For full flexibility, we need to use serde_json, given
            // crossterm's approach.
            serde_json::from_str::<Color>(format!("\"{}\"", &name[1..]).as_str())
                .map_err(|_| format!("Could not convert color name {name} to Crossterm color"))
        }
        _ => {
            let srgb = named::from_str(name).ok_or("No such color in palette")?;
            Ok(Color::Rgb {
                r: srgb.red,
                g: srgb.green,
                b: srgb.blue,
            })
        }
    }
}

pub struct StyleFactory {}

impl StyleFactory {
    fn from_fg_string(name: &str) -> Result<ContentStyle, String> {
        match from_string(name) {
            Ok(color) => Ok(Self::from_fg_color(color)),
            Err(err) => Err(err),
        }
    }

    // For succinctness, if we are confident that the name will be known,
    // this routine is available to keep the code readable
    fn known_fg_string(name: &str) -> ContentStyle {
        Self::from_fg_string(name).unwrap()
    }

    fn from_fg_color(color: Color) -> ContentStyle {
        ContentStyle {
            foreground_color: Some(color),
            ..ContentStyle::default()
        }
    }

    fn from_fg_color_and_attributes(color: Color, attributes: Attributes) -> ContentStyle {
        ContentStyle {
            foreground_color: Some(color),
            attributes,
            ..ContentStyle::default()
        }
    }
}

// Built-in themes. Rather than having extra files added before any theming
// is available, this gives a couple of basic options, demonstrating the use
// of themes: autumn, marine and pumpkin
static ALERT_TYPES: LazyLock<HashMap<tracing::Level, Meaning>> = LazyLock::new(|| {
    HashMap::from([
        (tracing::Level::INFO, Meaning::AlertInfo),
        (tracing::Level::WARN, Meaning::AlertWarn),
        (tracing::Level::ERROR, Meaning::AlertError),
    ])
});

static MEANING_FALLBACKS: LazyLock<HashMap<Meaning, Meaning>> = LazyLock::new(|| {
    HashMap::from([
        (Meaning::Guidance, Meaning::AlertInfo),
        (Meaning::Annotation, Meaning::AlertInfo),
        (Meaning::Title, Meaning::Important),
        (Meaning::SyntaxComment, Meaning::Annotation),
    ])
});

static DEFAULT_THEME: LazyLock<Theme> = LazyLock::new(|| {
    Theme::new(
        "default".to_string(),
        None,
        HashMap::from([
            (Meaning::AlertError, StyleFactory::from_fg_color(Color::DarkRed)),
            (Meaning::AlertWarn, StyleFactory::from_fg_color(Color::DarkYellow)),
            (Meaning::AlertInfo, StyleFactory::from_fg_color(Color::DarkGreen)),
            (Meaning::Annotation, StyleFactory::known_fg_string("steelblue")),
            (Meaning::Guidance, StyleFactory::from_fg_color(Color::DarkBlue)),
            (
                Meaning::Important,
                StyleFactory::from_fg_color_and_attributes(
                    Color::White,
                    Attributes::from(Attribute::Bold),
                ),
            ),
            (Meaning::Muted, StyleFactory::from_fg_color(Color::Grey)),
            (Meaning::Base, ContentStyle::default()),
            // Syntax highlighting uses ANSI palette colors, so they follow
            // the user's terminal color scheme out of the box.
            (Meaning::SyntaxCommand, StyleFactory::from_fg_color(Color::Green)),
            (Meaning::SyntaxFlag, StyleFactory::from_fg_color(Color::DarkCyan)),
            (Meaning::SyntaxString, StyleFactory::from_fg_color(Color::DarkYellow)),
            (Meaning::SyntaxVariable, StyleFactory::from_fg_color(Color::Magenta)),
            // Operators keep the terminal's default foreground
            (Meaning::SyntaxOperator, ContentStyle::default()),
            (Meaning::SyntaxComment, StyleFactory::from_fg_color(Color::DarkGrey)),
        ]),
    )
});

static BUILTIN_THEMES: LazyLock<HashMap<&'static str, Theme>> = LazyLock::new(|| {
    HashMap::from([
        ("default", HashMap::new()),
        (
            "(none)",
            DEFAULT_THEME
                .styles
                .keys()
                .map(|&meaning| (meaning, ContentStyle::default()))
                .collect(),
        ),
        (
            "autumn",
            HashMap::from([
                (Meaning::AlertError, StyleFactory::known_fg_string("saddlebrown")),
                (Meaning::AlertWarn, StyleFactory::known_fg_string("darkorange")),
                (Meaning::AlertInfo, StyleFactory::known_fg_string("gold")),
                (Meaning::Annotation, StyleFactory::known_fg_string("steelblue")),
                (Meaning::Guidance, StyleFactory::known_fg_string("brown")),
            ]),
        ),
        (
            "marine",
            HashMap::from([
                (Meaning::AlertError, StyleFactory::known_fg_string("yellowgreen")),
                (Meaning::AlertWarn, StyleFactory::known_fg_string("cyan")),
                (Meaning::AlertInfo, StyleFactory::known_fg_string("turquoise")),
                (Meaning::Annotation, StyleFactory::known_fg_string("steelblue")),
                (Meaning::Base, StyleFactory::known_fg_string("lightsteelblue")),
                (Meaning::Guidance, StyleFactory::known_fg_string("teal")),
            ]),
        ),
        ("pumpkin", {
            // Colors from the Dracula palette (MIT). Base stays unset so the theme reads on light
            // terminals too.
            const ORANGE: &str = "#ffb86c";
            const PURPLE: &str = "#bd93f9";
            const GREEN: &str = "#50fa7b";
            const RED: &str = "#ff5555";
            const PINK: &str = "#ff79c6";
            const COMMENT: &str = "#6272a4";
            HashMap::from([
                (Meaning::AlertError, StyleFactory::known_fg_string(RED)),
                (Meaning::AlertWarn, StyleFactory::known_fg_string(ORANGE)),
                (Meaning::AlertInfo, StyleFactory::known_fg_string(GREEN)),
                (Meaning::Annotation, StyleFactory::known_fg_string(COMMENT)),
                (Meaning::Guidance, StyleFactory::known_fg_string(PURPLE)),
                (Meaning::Important, ContentStyle {
                    attributes: Attributes::from(Attribute::Bold),
                    ..StyleFactory::known_fg_string(ORANGE)
                }),
                (Meaning::SyntaxCommand, StyleFactory::known_fg_string(ORANGE)),
                (Meaning::SyntaxFlag, StyleFactory::known_fg_string(PURPLE)),
                (Meaning::SyntaxString, StyleFactory::known_fg_string(GREEN)),
                (Meaning::SyntaxVariable, StyleFactory::known_fg_string(PINK)),
                (Meaning::SyntaxComment, StyleFactory::known_fg_string(COMMENT)),
            ])
        }),
        (
            // This is the default theme with default PowerShell syntax highlighting colors.
            "default-powershell",
            HashMap::from([
                (Meaning::SyntaxCommand, StyleFactory::from_fg_color(Color::Yellow)),
                (Meaning::SyntaxFlag, StyleFactory::from_fg_color(Color::DarkGrey)),
                (Meaning::SyntaxString, StyleFactory::from_fg_color(Color::DarkCyan)),
                (Meaning::SyntaxVariable, StyleFactory::from_fg_color(Color::Green)),
                (Meaning::SyntaxOperator, StyleFactory::from_fg_color(Color::DarkGrey)),
                (Meaning::SyntaxComment, StyleFactory::from_fg_color(Color::DarkGreen)),
            ]),
        ),
    ])
    .iter()
    .map(|(name, theme)| (*name, Theme::from_map(name.to_string(), None, theme)))
    .collect()
});

/// Loads themes by name and keeps each one loaded, so that it is read only once.
///
/// The default manager reads theme files from the user's themes directory.
#[derive(Default)]
pub struct ThemeManager {
    loaded_themes: HashMap<String, Theme>,
    /// Only load built-in themes, rather than reading the user's theme directory.
    builtin_only: bool,
}

// Theme-loading logic
impl ThemeManager {
    /// Most parents that loading one theme follows, which is far more than any real theme has.
    const MAX_PARENTS: usize = 8192;

    /// A manager that only loads built-in themes and never reads the themes directory.
    #[must_use]
    pub fn builtin_only() -> Self {
        Self {
            builtin_only: true,
            ..Self::default()
        }
    }

    /// The theme called `name`, loading it and its parents from the themes directory if it is
    /// neither loaded nor built in.
    ///
    /// A theme that fails to load is logged and replaced by the `(none)` theme.
    #[must_use]
    pub fn load_theme(&mut self, name: &str) -> &Theme {
        // `if let Some(theme) = self.get(name) { return theme; }` keeps `self` borrowed for the
        // load below, so the lookup runs twice.
        if self.get(name).is_some() {
            return self.get(name).expect("the theme was found just above");
        }
        match self.read_theme_file(name) {
            Ok(config) => self.load_with_parents(name, config),
            Err(err) => {
                userio::log::warn!("theme-load-failed", name = name, error = err.to_string());
                &BUILTIN_THEMES["(none)"]
            }
        }
    }

    /// Load the theme in `config` as `name`, with the parents that it names.
    pub fn load_theme_from_config(
        &mut self,
        name: &str,
        config: Config,
    ) -> Result<&Theme, Box<dyn error::Error>> {
        let config = ThemeConfig::parse(config)?;
        Ok(self.load_with_parents(name, config))
    }

    fn get(&self, name: &str) -> Option<&Theme> {
        self.loaded_themes.get(name).or_else(|| BUILTIN_THEMES.get(name))
    }

    /// Walks up from `config` to the first parent that is loaded or built in, then builds the
    /// themes on the way back down, so that each one starts from its parent's styles.
    ///
    /// A parent that fails to load is logged and replaced by the `(none)` theme.
    fn load_with_parents(&mut self, name: &str, config: ThemeConfig) -> &Theme {
        let mut chain = vec![(name.to_owned(), config)];
        let mut base = loop {
            let (_, config) = chain.last().expect("the chain starts with `name`");
            let parent = config.theme.parent.clone().unwrap_or_else(|| "default".to_owned());
            if self.get(&parent).is_some() {
                break parent;
            }
            let parent_config = if chain.iter().any(|(key, _)| *key == parent) {
                Err(format!("theme {parent} is its own ancestor").into())
            } else if chain.len() > Self::MAX_PARENTS {
                Err(format!("theme {name} has more than {} parents", Self::MAX_PARENTS).into())
            } else {
                self.read_theme_file(&parent)
            };
            match parent_config {
                Ok(config) => chain.push((parent, config)),
                Err(err) => {
                    userio::log::warn!(
                        "theme-load-failed",
                        name = parent.as_str(),
                        error = err.to_string()
                    );
                    break "(none)".to_owned();
                }
            }
        };

        for (key, config) in chain.into_iter().rev() {
            let theme =
                Theme::from_foreground_colors(config.theme.name, self.get(&base), &config.colors);
            self.loaded_themes.insert(key.clone(), theme);
            base = key;
        }
        &self.loaded_themes[name]
    }

    /// Reads `{name}.toml` from the themes directory, unless only built-in themes are enabled.
    fn read_theme_file(&self, name: &str) -> Result<ThemeConfig, Box<dyn error::Error>> {
        if self.builtin_only {
            return Err("cannot load theme file: only built-in themes are enabled".into());
        }
        let theme_file = atuin_common::dirs::config_path("themes").joined(format!("{name}.toml"));
        let config = Config::builder()
            .add_source(ConfigFile::from(theme_file).format(FileFormat::Toml))
            .build()?;
        ThemeConfig::parse(config)
    }
}

#[cfg(test)]
mod theme_tests {
    use atuin_common::env::MockEnv;
    use atuin_common::test_utils::capture_logs;
    use rstest::*;
    use tempfile::TempDir;

    use super::*;

    #[fixture]
    fn manager() -> ThemeManager {
        ThemeManager::builtin_only()
    }

    /// A mock environment whose `ATUIN_HOME` is a temporary directory with an empty `themes`
    /// directory.
    struct ThemesDir {
        home: TempDir,
        _env: MockEnv,
    }

    impl ThemesDir {
        fn write(&self, name: &str, toml: &str) {
            std::fs::write(self.home.path().join("themes").joined(format!("{name}.toml")), toml)
                .unwrap();
        }
    }

    #[fixture]
    fn themes_dir() -> ThemesDir {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("themes")).unwrap();
        let env = MockEnv::install();
        env.set("ATUIN_HOME", home.path());
        ThemesDir { home, _env: env }
    }

    fn theme_config(toml: &str) -> Config {
        Config::builder().add_source(ConfigFile::from_str(toml, FileFormat::Toml)).build().unwrap()
    }

    const XINETIC_WARNING: &str = "tried to load string as a color unsuccessfully: \
                                   (AlertWarn=xinetic) No such color in palette";

    /// A theme file inherits from the theme files that it names as parents, down to a built-in
    /// theme. A cycle of parents stops at the first theme that repeats, with a warning. It keeps
    /// the colors of every theme in the cycle, builds each theme once, and falls back to the
    /// `(none)` theme.
    #[rstest]
    #[case::chain("", Some(Color::DarkRed), &[XINETIC_WARNING])]
    #[case::cycle(
        "parent = \"a\"",
        None,
        &["could not load theme a: theme a is its own ancestor", XINETIC_WARNING],
    )]
    #[case::own_parent(
        "parent = \"b\"",
        None,
        &["could not load theme b: theme b is its own ancestor", XINETIC_WARNING],
    )]
    fn loads_parents_from_theme_files(
        themes_dir: ThemesDir,
        #[case] b_parent: &str,
        #[case] alert_error: Option<Color>,
        #[case] warnings: &[&str],
    ) {
        let logs = capture_logs();
        themes_dir.write(
            "a",
            "[theme]\nname = \"a\"\nparent = \"b\"\n[colors]\nBase = \"red\"\nAlertWarn = \
             \"xinetic\"\n",
        );
        themes_dir.write(
            "b",
            &format!("[theme]\nname = \"b\"\n{b_parent}\n[colors]\nGuidance = \"blue\"\n"),
        );
        let mut manager = ThemeManager::default();

        let theme = manager.load_theme("a");

        assert_eq!(theme.as_style(Meaning::Base).foreground_color, from_string("red").ok());
        assert_eq!(theme.as_style(Meaning::Guidance).foreground_color, from_string("blue").ok());
        assert_eq!(theme.as_style(Meaning::AlertError).foreground_color, alert_error);
        let messages: Vec<_> = logs.get().iter().map(|log| log.message.clone()).collect();
        assert_eq!(messages, warnings);
    }

    #[rstest]
    #[case::autumn("autumn", Meaning::Guidance, "brown")]
    #[case::pumpkin("pumpkin", Meaning::Important, "#ffb86c")]
    fn test_can_load_builtin_theme(
        mut manager: ThemeManager,
        #[case] name: &str,
        #[case] meaning: Meaning,
        #[case] color: &str,
    ) {
        let theme = manager.load_theme(name);
        assert_eq!(theme.as_style(meaning).foreground_color, from_string(color).ok());
    }

    /// The warning for a theme file that does not parse says why.
    #[rstest]
    fn reports_why_a_theme_file_does_not_parse(themes_dir: ThemesDir) {
        let logs = capture_logs();
        themes_dir.write("broken", "[theme]\nname = \"broken\"\n");
        let mut manager = ThemeManager::default();

        let _ = manager.load_theme("broken");

        let messages: Vec<_> = logs.get().iter().map(|log| log.message.clone()).collect();
        assert_eq!(messages, ["could not load theme broken: Failed to deserialize theme: \
                               missing configuration field \"colors\""]);
    }

    // Annotation is used for the directory/user/host columns in the history popup, so it
    // needs to stay readable on dark terminal backgrounds. DarkGrey was too close to the
    // background on those terminals (see #3539), so default and autumn should use the same
    // steelblue that marine already uses.
    #[rstest]
    fn test_annotation_has_contrast_in_builtin_themes(mut manager: ThemeManager) {
        for name in ["default", "autumn", "marine"] {
            let theme = manager.load_theme(name, None);
            assert_eq!(
                theme.as_style(Meaning::Annotation).foreground_color,
                from_string("steelblue").ok(),
                "unexpected Annotation color for {name} theme"
            );
        }
    }

    #[rstest]
    fn test_can_create_theme(mut manager: ThemeManager) {
        let mytheme = Theme::new(
            "mytheme".to_string(),
            None,
            HashMap::from([(Meaning::AlertError, StyleFactory::known_fg_string("yellowgreen"))]),
        );
        manager.loaded_themes.insert("mytheme".to_string(), mytheme);
        let theme = manager.load_theme("mytheme");
        assert_eq!(
            theme.as_style(Meaning::AlertError).foreground_color,
            from_string("yellowgreen").ok()
        );
    }

    #[rstest]
    fn test_can_fallback_when_meaning_missing(mut manager: ThemeManager) {
        // We use title as an example of a meaning that is not defined
        // even in the base theme.
        assert!(!DEFAULT_THEME.styles.contains_key(&Meaning::Title));

        let config = theme_config(
            "
        [theme]
        name = \"title_theme\"

        [colors]
        Guidance = \"white\"
        AlertInfo = \"zomp\"
        ",
        );
        let theme = manager.load_theme_from_config("config_theme", config).unwrap();

        // Correctly picks overridden color.
        assert_eq!(theme.as_style(Meaning::Guidance).foreground_color, from_string("white").ok());

        // Does not fall back to any color.
        assert_eq!(theme.as_style(Meaning::AlertInfo).foreground_color, None);

        // Even for the base.
        assert_eq!(theme.as_style(Meaning::Base).foreground_color, None);

        // Falls back to red as meaning missing from theme, so picks base default.
        assert_eq!(theme.as_style(Meaning::AlertError).foreground_color, Some(Color::DarkRed));

        // Falls back to Important as Title not available.
        assert_eq!(
            theme.as_style(Meaning::Title).foreground_color,
            theme.as_style(Meaning::Important).foreground_color,
        );

        let title_config = theme_config(
            "
        [theme]
        name = \"title_theme\"

        [colors]
        Title = \"white\"
        AlertInfo = \"zomp\"
        ",
        );
        let title_theme = manager.load_theme_from_config("title_theme", title_config).unwrap();

        assert_eq!(title_theme.as_style(Meaning::Title).foreground_color, Some(Color::White));
    }

    #[rstest]
    fn test_no_fallbacks_are_circular() {
        let mytheme = Theme::new("mytheme".to_string(), None, HashMap::from([]));
        MEANING_FALLBACKS
            .iter()
            .for_each(|pair| assert_eq!(mytheme.closest_meaning(pair.0), &Meaning::Base));
    }

    #[rstest]
    fn test_can_get_colors_via_convenience_functions(mut manager: ThemeManager) {
        let theme = manager.load_theme("default");
        assert_eq!(theme.get_error().foreground_color.unwrap(), Color::DarkRed);
        assert_eq!(theme.get_warning().foreground_color.unwrap(), Color::DarkYellow);
        assert_eq!(theme.get_info().foreground_color.unwrap(), Color::DarkGreen);
        assert_eq!(theme.get_base().foreground_color, None);
        assert_eq!(
            theme.get_alert(tracing::Level::ERROR).foreground_color.unwrap(),
            Color::DarkRed
        );
    }

    #[rstest]
    fn test_can_use_parent_theme_for_fallbacks(mut manager: ThemeManager) {
        let logs = capture_logs();

        // First, we introduce a base theme
        let solarized = theme_config(
            "
        [theme]
        name = \"solarized\"

        [colors]
        Guidance = \"white\"
        AlertInfo = \"pink\"
        ",
        );
        let solarized_theme = manager.load_theme_from_config("solarized", solarized).unwrap();

        assert_eq!(
            solarized_theme.as_style(Meaning::AlertInfo).foreground_color,
            from_string("pink").ok()
        );

        // Then we introduce a derived theme
        let unsolarized = theme_config(
            "
        [theme]
        name = \"unsolarized\"
        parent = \"solarized\"

        [colors]
        AlertInfo = \"red\"
        ",
        );
        let unsolarized_theme = manager.load_theme_from_config("unsolarized", unsolarized).unwrap();

        // It will take its own values
        assert_eq!(
            unsolarized_theme.as_style(Meaning::AlertInfo).foreground_color,
            from_string("red").ok()
        );

        // ...or fall back to the parent
        assert_eq!(
            unsolarized_theme.as_style(Meaning::Guidance).foreground_color,
            from_string("white").ok()
        );

        assert_eq!(logs.get().len(), 0);

        // If the parent is not found, we end up with the no theme colors or styling
        // as this is considered a (soft) error state.
        let nunsolarized = theme_config(
            "
        [theme]
        name = \"nunsolarized\"
        parent = \"nonsolarized\"

        [colors]
        AlertInfo = \"red\"
        ",
        );
        let nunsolarized_theme =
            manager.load_theme_from_config("nunsolarized", nunsolarized).unwrap();

        assert_eq!(nunsolarized_theme.as_style(Meaning::Guidance).foreground_color, None);

        let captured_logs = logs.get();
        assert_eq!(captured_logs.len(), 1);
        assert_eq!(
            captured_logs[0].message,
            "could not load theme nonsolarized: cannot load theme file: only built-in themes are \
             enabled"
        );
        assert_eq!(captured_logs[0].level, tracing::Level::WARN);
    }

    /// A color name that does not parse is logged and left unstyled. A theme whose own `name`
    /// differs from the name that it is loaded as loads without a warning.
    #[rstest]
    fn logs_invalid_colors(mut manager: ThemeManager) {
        let logs = capture_logs();
        let config = theme_config(
            "[theme]\nname = \"mytheme\"\n[colors]\nGuidance = \"white\"\nAlertInfo = \
             \"xinetic\"\n",
        );

        let theme = manager.load_theme_from_config("config_theme", config).unwrap();

        assert_eq!(theme.as_style(Meaning::AlertInfo).foreground_color, None);
        let messages: Vec<_> = logs.get().iter().map(|log| log.message.clone()).collect();
        assert_eq!(messages, ["tried to load string as a color unsuccessfully: \
                               (AlertInfo=xinetic) No such color in palette"]);
    }

    #[rstest]
    #[case::palette_name("brown", Color::Rgb { r: 165, g: 42, b: 42 })]
    #[case::hex("#ff1122", Color::Rgb { r: 255, g: 17, b: 34 })]
    #[case::at_named("@dark_grey", Color::DarkGrey)]
    #[case::at_rgb("@rgb_(255,255,255)", Color::Rgb { r: 255, g: 255, b: 255 })]
    #[case::at_ansi("@ansi_(255)", Color::AnsiValue(255))]
    fn parses_color_string(#[case] input: &str, #[case] expected: Color) {
        assert_eq!(from_string(input).unwrap(), expected);
    }

    #[rstest]
    #[case::empty("", "Empty string")]
    #[case::not_in_palette_word("manatee", "No such color in palette")]
    #[case::not_in_palette_phrase("caput mortuum", "No such color in palette")]
    #[case::not_in_palette_digits("123456", "No such color in palette")]
    #[case::hex_too_short("#1122", "Could not parse 3 hex values from string")]
    #[case::hex_too_long("#ffaa112", "Could not parse 3 hex values from string")]
    #[case::hex_not_hex("#brown", "Could not parse 3 hex values from string")]
    fn rejects_color_string(#[case] input: &str, #[case] expected_err: &str) {
        assert_eq!(from_string(input), Err(expected_err.into()));
    }

    /// The `@`-prefixed forms echo the offending input back in the error message.
    #[rstest]
    #[case::bare_at("@")]
    #[case::wrong_case("@DarkGray")]
    #[case::malformed_name("@Dark 4ay")]
    #[case::ansi_out_of_range("@ansi(256)")]
    fn rejects_at_prefixed_color_string(#[case] input: &str) {
        assert_eq!(
            from_string(input),
            Err(format!("Could not convert color name {input} to Crossterm color"))
        );
    }
}
