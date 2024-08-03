use config::{Config, File as ConfigFile, FileFormat};
use lazy_static::lazy_static;
use log;
use palette::named;
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::HashMap;
use std::error;
use std::io::{Error, ErrorKind};
use std::path::PathBuf;
use strum_macros;

static DEFAULT_MAX_DEPTH: u8 = 10;

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
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ThemeConfig {
    // Definition of the theme
    pub theme: ThemeDefinitionConfigBlock,

    // Colors
    pub colors: HashMap<Meaning, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ThemeDefinitionConfigBlock {
    /// Name of theme ("default" for base)
    pub name: String,

    /// Whether any theme should be treated as a parent _if available_
    pub parent: Option<String>,
}

use crossterm::style::{Color, ContentStyle};

// For now, a theme is specifically a mapping of meanings to colors, but it may be desirable to
// expand that in the future to general styles.
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
    pub fn get_base(&self) -> ContentStyle {
        self.styles[&Meaning::Base]
    }

    pub fn get_info(&self) -> ContentStyle {
        self.get_alert(log::Level::Info)
    }

    pub fn get_warning(&self) -> ContentStyle {
        self.get_alert(log::Level::Warn)
    }

    pub fn get_error(&self) -> ContentStyle {
        self.get_alert(log::Level::Error)
    }

    // The alert meanings may be chosen by the Level enum, rather than the methods above
    // or the full Meaning enum, to simplify programmatic selection of a log-level.
    pub fn get_alert(&self, severity: log::Level) -> ContentStyle {
        self.styles[ALERT_TYPES.get(&severity).unwrap()]
    }

    pub fn new(
        name: String,
        parent: Option<String>,
        styles: HashMap<Meaning, ContentStyle>,
    ) -> Theme {
        Theme {
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
    pub fn as_style(&self, meaning: Meaning) -> ContentStyle {
        self.styles[self.closest_meaning(&meaning)]
    }

    // Turns a map of meanings to colornames into a theme
    // If theme-debug is on, then we will print any colornames that we cannot load,
    // but we do not have this on in general, as it could print unfiltered text to the terminal
    // from a theme TOML file. However, it will always return a theme, falling back to
    // defaults on error, so that a TOML file does not break loading
    pub fn from_foreground_colors(
        name: String,
        parent: Option<&Theme>,
        foreground_colors: HashMap<Meaning, String>,
        debug: bool,
    ) -> Theme {
        let styles: HashMap<Meaning, ContentStyle> = foreground_colors
            .iter()
            .map(|(name, color)| {
                (
                    *name,
                    StyleFactory::from_fg_string(color).unwrap_or_else(|err| {
                        if debug {
                            log::warn!(
                                "Tried to load string as a color unsuccessfully: ({}={}) {}",
                                name,
                                color,
                                err
                            );
                        }
                        ContentStyle::default()
                    }),
                )
            })
            .collect();
        Theme::from_map(name, parent, &styles)
    }

    // Boil down a meaning-color hashmap into a theme, by taking the defaults
    // for any unknown colors
    fn from_map(
        name: String,
        parent: Option<&Theme>,
        overrides: &HashMap<Meaning, ContentStyle>,
    ) -> Theme {
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
        Theme::new(name, parent.map(|p| p.name.clone()), styles)
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
                .map_err(|_| format!("Could not convert color name {} to Crossterm color", name))
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
}

// Built-in themes. Rather than having extra files added before any theming
// is available, this gives a couple of basic options, demonstrating the use
// of themes: autumn and marine
lazy_static! {
    static ref ALERT_TYPES: HashMap<log::Level, Meaning> = {
        HashMap::from([
            (log::Level::Info, Meaning::AlertInfo),
            (log::Level::Warn, Meaning::AlertWarn),
            (log::Level::Error, Meaning::AlertError),
        ])
    };
    static ref MEANING_FALLBACKS: HashMap<Meaning, Meaning> = {
        HashMap::from([
            (Meaning::Guidance, Meaning::AlertInfo),
            (Meaning::Annotation, Meaning::AlertInfo),
            (Meaning::Title, Meaning::Important),
        ])
    };
    static ref DEFAULT_THEME: Theme = {
        Theme::new(
            "default".to_string(),
            None,
            HashMap::from([
                (
                    Meaning::AlertError,
                    StyleFactory::from_fg_color(Color::DarkRed),
                ),
                (
                    Meaning::AlertWarn,
                    StyleFactory::from_fg_color(Color::DarkYellow),
                ),
                (
                    Meaning::AlertInfo,
                    StyleFactory::from_fg_color(Color::DarkGreen),
                ),
                (
                    Meaning::Annotation,
                    StyleFactory::from_fg_color(Color::DarkGrey),
                ),
                (
                    Meaning::Guidance,
                    StyleFactory::from_fg_color(Color::DarkBlue),
                ),
                (
                    Meaning::Important,
                    StyleFactory::from_fg_color(Color::White),
                ),
                (Meaning::Muted, StyleFactory::from_fg_color(Color::Grey)),
                (Meaning::Base, ContentStyle::default()),
            ]),
        )
    };
    static ref BUILTIN_THEMES: HashMap<&'static str, Theme> = {
        HashMap::from([
            ("default", HashMap::new()),
            (
                "autumn",
                HashMap::from([
                    (
                        Meaning::AlertError,
                        StyleFactory::known_fg_string("saddlebrown"),
                    ),
                    (
                        Meaning::AlertWarn,
                        StyleFactory::known_fg_string("darkorange"),
                    ),
                    (Meaning::AlertInfo, StyleFactory::known_fg_string("gold")),
                    (
                        Meaning::Annotation,
                        StyleFactory::from_fg_color(Color::DarkGrey),
                    ),
                    (Meaning::Guidance, StyleFactory::known_fg_string("brown")),
                ]),
            ),
            (
                "marine",
                HashMap::from([
                    (
                        Meaning::AlertError,
                        StyleFactory::known_fg_string("yellowgreen"),
                    ),
                    (Meaning::AlertWarn, StyleFactory::known_fg_string("cyan")),
                    (
                        Meaning::AlertInfo,
                        StyleFactory::known_fg_string("turquoise"),
                    ),
                    (
                        Meaning::Annotation,
                        StyleFactory::known_fg_string("steelblue"),
                    ),
                    (
                        Meaning::Base,
                        StyleFactory::known_fg_string("lightsteelblue"),
                    ),
                    (Meaning::Guidance, StyleFactory::known_fg_string("teal")),
                ]),
            ),
        ])
        .iter()
        .map(|(name, theme)| (*name, Theme::from_map(name.to_string(), None, theme)))
        .collect()
    };
}

// To avoid themes being repeatedly loaded, we store them in a theme manager
pub struct ThemeManager {
    loaded_themes: HashMap<String, Theme>,
    debug: bool,
    override_theme_dir: Option<String>,
}

// Theme-loading logic
impl ThemeManager {
    pub fn new(debug: Option<bool>, theme_dir: Option<String>) -> Self {
        Self {
            loaded_themes: HashMap::new(),
            debug: debug.unwrap_or(false),
            override_theme_dir: match theme_dir {
                Some(theme_dir) => Some(theme_dir),
                None => std::env::var("ATUIN_THEME_DIR").ok(),
            },
        }
    }

    // Try to load a theme from a `{name}.toml` file in the theme directory. If an override is set
    // for the theme dir (via ATUIN_THEME_DIR env) we should load the theme from there
    pub fn load_theme_from_file(
        &mut self,
        name: &str,
        max_depth: u8,
    ) -> Result<&Theme, Box<dyn error::Error>> {
        let mut theme_file = if let Some(p) = &self.override_theme_dir {
            if p.is_empty() {
                return Err(Box::new(Error::new(
                    ErrorKind::NotFound,
                    "Empty theme directory override and could not find theme elsewhere",
                )));
            }
            PathBuf::from(p)
        } else {
            let config_dir = atuin_common::utils::config_dir();
            let mut theme_file = PathBuf::new();
            theme_file.push(config_dir);
            theme_file.push("themes");
            theme_file
        };

        let theme_toml = format!["{}.toml", name];
        theme_file.push(theme_toml);

        let mut config_builder = Config::builder();

        config_builder = config_builder.add_source(ConfigFile::new(
            theme_file.to_str().unwrap(),
            FileFormat::Toml,
        ));

        let config = config_builder.build()?;
        self.load_theme_from_config(name, config, max_depth)
    }

    pub fn load_theme_from_config(
        &mut self,
        name: &str,
        config: Config,
        max_depth: u8,
    ) -> Result<&Theme, Box<dyn error::Error>> {
        let debug = self.debug;
        let theme_config: ThemeConfig = match config.try_deserialize() {
            Ok(tc) => tc,
            Err(e) => {
                return Err(Box::new(Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "Failed to deserialize theme: {}",
                        if debug {
                            e.to_string()
                        } else {
                            "set theme debug on for more info".to_string()
                        }
                    ),
                )))
            }
        };
        let colors: HashMap<Meaning, String> = theme_config.colors;
        let parent: Option<&Theme> = match theme_config.theme.parent {
            Some(parent_name) => {
                if max_depth == 0 {
                    return Err(Box::new(Error::new(
                        ErrorKind::InvalidInput,
                        "Parent requested but we hit the recursion limit",
                    )));
                }
                Some(self.load_theme(parent_name.as_str(), Some(max_depth - 1)))
            }
            None => None,
        };

        if debug && name != theme_config.theme.name {
            log::warn!(
                "Your theme config name is not the name of your loaded theme {} != {}",
                name,
                theme_config.theme.name
            );
        }

        let theme = Theme::from_foreground_colors(theme_config.theme.name, parent, colors, debug);
        let name = name.to_string();
        self.loaded_themes.insert(name.clone(), theme);
        let theme = self.loaded_themes.get(&name).unwrap();
        Ok(theme)
    }

    // Check if the requested theme is loaded and, if not, then attempt to get it
    // from the builtins or, if not there, from file
    pub fn load_theme(&mut self, name: &str, max_depth: Option<u8>) -> &Theme {
        if self.loaded_themes.contains_key(name) {
            return self.loaded_themes.get(name).unwrap();
        }
        let built_ins = &BUILTIN_THEMES;
        match built_ins.get(name) {
            Some(theme) => theme,
            None => match self.load_theme_from_file(name, max_depth.unwrap_or(DEFAULT_MAX_DEPTH)) {
                Ok(theme) => theme,
                Err(err) => {
                    log::warn!("Could not load theme {}: {}", name, err);
                    built_ins.get("default").unwrap()
                }
            },
        }
    }
}

#[cfg(test)]
mod theme_tests {
    use super::*;

    #[test]
    fn test_can_load_builtin_theme() {
        let mut manager = ThemeManager::new(Some(false), Some("".to_string()));
        let theme = manager.load_theme("autumn", None);
        assert_eq!(
            theme.as_style(Meaning::Guidance).foreground_color,
            from_string("brown").ok()
        );
    }

    #[test]
    fn test_can_create_theme() {
        let mut manager = ThemeManager::new(Some(false), Some("".to_string()));
        let mytheme = Theme::new(
            "mytheme".to_string(),
            None,
            HashMap::from([(
                Meaning::AlertError,
                StyleFactory::known_fg_string("yellowgreen"),
            )]),
        );
        manager.loaded_themes.insert("mytheme".to_string(), mytheme);
        let theme = manager.load_theme("mytheme", None);
        assert_eq!(
            theme.as_style(Meaning::AlertError).foreground_color,
            from_string("yellowgreen").ok()
        );
    }

    #[test]
    fn test_can_fallback_when_meaning_missing() {
        let mut manager = ThemeManager::new(Some(false), Some("".to_string()));

        // We use title as an example of a meaning that is not defined
        // even in the base theme.
        assert!(!DEFAULT_THEME.styles.contains_key(&Meaning::Title));

        let config = Config::builder()
            .add_source(ConfigFile::from_str(
                "
        [theme]
        name = \"title_theme\"

        [colors]
        Guidance = \"white\"
        AlertInfo = \"zomp\"
        ",
                FileFormat::Toml,
            ))
            .build()
            .unwrap();
        let theme = manager
            .load_theme_from_config("config_theme", config, 1)
            .unwrap();

        // Correctly picks overridden color.
        assert_eq!(
            theme.as_style(Meaning::Guidance).foreground_color,
            from_string("white").ok()
        );

        // Does not fall back to any color.
        assert_eq!(theme.as_style(Meaning::AlertInfo).foreground_color, None);

        // Even for the base.
        assert_eq!(theme.as_style(Meaning::Base).foreground_color, None);

        // Falls back to red as meaning missing from theme, so picks base default.
        assert_eq!(
            theme.as_style(Meaning::AlertError).foreground_color,
            Some(Color::DarkRed)
        );

        // Falls back to Important as Title not available.
        assert_eq!(
            theme.as_style(Meaning::Title).foreground_color,
            theme.as_style(Meaning::Important).foreground_color,
        );

        let title_config = Config::builder()
            .add_source(ConfigFile::from_str(
                "
        [theme]
        name = \"title_theme\"

        [colors]
        Title = \"white\"
        AlertInfo = \"zomp\"
        ",
                FileFormat::Toml,
            ))
            .build()
            .unwrap();
        let title_theme = manager
            .load_theme_from_config("title_theme", title_config, 1)
            .unwrap();

        assert_eq!(
            title_theme.as_style(Meaning::Title).foreground_color,
            Some(Color::White)
        );
    }

    #[test]
    fn test_no_fallbacks_are_circular() {
        let mytheme = Theme::new("mytheme".to_string(), None, HashMap::from([]));
        MEANING_FALLBACKS
            .iter()
            .for_each(|pair| assert_eq!(mytheme.closest_meaning(pair.0), &Meaning::Base))
    }

    #[test]
    fn test_can_get_colors_via_convenience_functions() {
        let mut manager = ThemeManager::new(Some(true), Some("".to_string()));
        let theme = manager.load_theme("default", None);
        assert_eq!(theme.get_error().foreground_color.unwrap(), Color::DarkRed);
        assert_eq!(
            theme.get_warning().foreground_color.unwrap(),
            Color::DarkYellow
        );
        assert_eq!(theme.get_info().foreground_color.unwrap(), Color::DarkGreen);
        assert_eq!(theme.get_base().foreground_color, None);
        assert_eq!(
            theme.get_alert(log::Level::Error).foreground_color.unwrap(),
            Color::DarkRed
        )
    }

    #[test]
    fn test_can_use_parent_theme_for_fallbacks() {
        testing_logger::setup();

        let mut manager = ThemeManager::new(Some(false), Some("".to_string()));

        // First, we introduce a base theme
        let solarized = Config::builder()
            .add_source(ConfigFile::from_str(
                "
        [theme]
        name = \"solarized\"

        [colors]
        Guidance = \"white\"
        AlertInfo = \"pink\"
        ",
                FileFormat::Toml,
            ))
            .build()
            .unwrap();
        let solarized_theme = manager
            .load_theme_from_config("solarized", solarized, 1)
            .unwrap();

        assert_eq!(
            solarized_theme
                .as_style(Meaning::AlertInfo)
                .foreground_color,
            from_string("pink").ok()
        );

        // Then we introduce a derived theme
        let unsolarized = Config::builder()
            .add_source(ConfigFile::from_str(
                "
        [theme]
        name = \"unsolarized\"
        parent = \"solarized\"

        [colors]
        AlertInfo = \"red\"
        ",
                FileFormat::Toml,
            ))
            .build()
            .unwrap();
        let unsolarized_theme = manager
            .load_theme_from_config("unsolarized", unsolarized, 1)
            .unwrap();

        // It will take its own values
        assert_eq!(
            unsolarized_theme
                .as_style(Meaning::AlertInfo)
                .foreground_color,
            from_string("red").ok()
        );

        // ...or fall back to the parent
        assert_eq!(
            unsolarized_theme
                .as_style(Meaning::Guidance)
                .foreground_color,
            from_string("white").ok()
        );

        testing_logger::validate(|captured_logs| assert_eq!(captured_logs.len(), 0));

        // If the parent is not found, we end up with the base theme colors
        let nunsolarized = Config::builder()
            .add_source(ConfigFile::from_str(
                "
        [theme]
        name = \"nunsolarized\"
        parent = \"nonsolarized\"

        [colors]
        AlertInfo = \"red\"
        ",
                FileFormat::Toml,
            ))
            .build()
            .unwrap();
        let nunsolarized_theme = manager
            .load_theme_from_config("nunsolarized", nunsolarized, 1)
            .unwrap();

        assert_eq!(
            nunsolarized_theme
                .as_style(Meaning::Guidance)
                .foreground_color,
            Some(Color::DarkBlue)
        );

        testing_logger::validate(|captured_logs| {
            assert_eq!(captured_logs.len(), 1);
            assert_eq!(captured_logs[0].body,
                "Could not load theme nonsolarized: Empty theme directory override and could not find theme elsewhere"
            );
            assert_eq!(captured_logs[0].level, log::Level::Warn)
        });
    }

    #[test]
    fn test_can_debug_theme() {
        testing_logger::setup();
        [true, false].iter().for_each(|debug| {
            let mut manager = ThemeManager::new(Some(*debug), Some("".to_string()));
            let config = Config::builder()
                .add_source(ConfigFile::from_str(
                    "
            [theme]
            name = \"mytheme\"

            [colors]
            Guidance = \"white\"
            AlertInfo = \"xinetic\"
            ",
                    FileFormat::Toml,
                ))
                .build()
                .unwrap();
            manager
                .load_theme_from_config("config_theme", config, 1)
                .unwrap();
            testing_logger::validate(|captured_logs| {
                if *debug {
                    assert_eq!(captured_logs.len(), 2);
                    assert_eq!(
                        captured_logs[0].body,
                        "Your theme config name is not the name of your loaded theme config_theme != mytheme"
                    );
                    assert_eq!(captured_logs[0].level, log::Level::Warn);
                    assert_eq!(
                        captured_logs[1].body,
                        "Tried to load string as a color unsuccessfully: (AlertInfo=xinetic) No such color in palette"
                    );
                    assert_eq!(captured_logs[1].level, log::Level::Warn)
                } else {
                    assert_eq!(captured_logs.len(), 0)
                }
            })
        })
    }

    #[test]
    fn test_can_parse_color_strings_correctly() {
        assert_eq!(
            from_string("brown").unwrap(),
            Color::Rgb {
                r: 165,
                g: 42,
                b: 42
            }
        );

        assert_eq!(from_string(""), Err("Empty string".into()));

        ["manatee", "caput mortuum", "123456"]
            .iter()
            .for_each(|inp| {
                assert_eq!(from_string(inp), Err("No such color in palette".into()));
            });

        assert_eq!(
            from_string("#ff1122").unwrap(),
            Color::Rgb {
                r: 255,
                g: 17,
                b: 34
            }
        );
        ["#1122", "#ffaa112", "#brown"].iter().for_each(|inp| {
            assert_eq!(
                from_string(inp),
                Err("Could not parse 3 hex values from string".into())
            );
        });

        assert_eq!(from_string("@dark_grey").unwrap(), Color::DarkGrey);
        assert_eq!(
            from_string("@rgb_(255,255,255)").unwrap(),
            Color::Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
        assert_eq!(from_string("@ansi_(255)").unwrap(), Color::AnsiValue(255));
        ["@", "@DarkGray", "@Dark 4ay", "@ansi(256)"]
            .iter()
            .for_each(|inp| {
                assert_eq!(
                    from_string(inp),
                    Err(format!(
                        "Could not convert color name {} to Crossterm color",
                        inp
                    ))
                );
            });
    }
}
