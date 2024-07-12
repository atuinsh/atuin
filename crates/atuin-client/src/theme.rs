use config::{Config, File as ConfigFile, FileFormat};
use itertools::Itertools;
use lazy_static::lazy_static;
use palette::named;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error;
use std::io::{Error, ErrorKind};
use std::path::PathBuf;
use strum_macros;

// Standard log-levels that may occur in the interface.
#[derive(
    Serialize, Deserialize, Copy, Clone, Hash, Debug, Eq, PartialEq, strum_macros::Display,
)]
#[strum(serialize_all = "camel_case")]
pub enum Level {
    Info,
    Warning,
    Error,
}

// Collection of settable "meanings" that can have colors set.
#[derive(
    Serialize, Deserialize, Copy, Clone, Hash, Debug, Eq, PartialEq, strum_macros::Display,
)]
#[strum(serialize_all = "camel_case")]
pub enum Meaning {
    AlertInfo,
    AlertWarning,
    AlertError,
    Annotation,
    Base,
    Guidance,
    Important,
}

use crossterm::style::{Color, ContentStyle};

// For now, a theme is specifically a mapping of meanings to colors, but it may be desirable to
// expand that in the future to general styles.
pub struct Theme {
    pub colors: HashMap<Meaning, Color>,
}

// Themes have a number of convenience functions for the most commonly used meanings.
// The general purpose `as_style` routine gives back a style, but for ease-of-use and to keep
// theme-related boilerplate minimal, the convenience functions give a color.
impl Theme {
    // This is the base "default" color, for general text
    pub fn get_base(&self) -> Color {
        self.colors[&Meaning::Base]
    }

    pub fn get_info(&self) -> Color {
        self.get_alert(Level::Info)
    }

    pub fn get_warning(&self) -> Color {
        self.get_alert(Level::Warning)
    }

    pub fn get_error(&self) -> Color {
        self.get_alert(Level::Error)
    }

    // The alert meanings may be chosen by the Level enum, rather than the methods above
    // or the full Meaning enum, to simplify programmatic selection of a log-level.
    pub fn get_alert(&self, severity: Level) -> Color {
        self.colors[ALERT_TYPES.get(&severity).unwrap()]
    }

    pub fn new(colors: HashMap<Meaning, Color>) -> Theme {
        Theme { colors }
    }

    // General access - if you have a meaning, this will give you a (crossterm) style
    pub fn as_style(&self, meaning: Meaning) -> ContentStyle {
        ContentStyle {
            foreground_color: Some(self.colors[&meaning]),
            ..ContentStyle::default()
        }
    }

    // Turns a map of meanings to colornames into a theme
    // If theme-debug is on, then we will print any colornames that we cannot load,
    // but we do not have this on in general, as it could print unfiltered text to the terminal
    // from a theme TOML file. However, it will always return a theme, falling back to
    // defaults on error, so that a TOML file does not break loading
    pub fn from_map(colors: HashMap<Meaning, String>, debug: bool) -> Theme {
        let colors: HashMap<Meaning, Color> = colors
            .iter()
            .map(|(name, color)| {
                (
                    *name,
                    from_string(color).unwrap_or_else(|msg: String| {
                        if debug {
                            println!["Could not load theme color: {} -> {}", msg, color];
                        }
                        Color::Grey
                    }),
                )
            })
            .collect();
        make_theme(&colors)
    }
}

// Use palette to get a color from a string name, if possible
fn from_string(name: &str) -> Result<Color, String> {
    if name.len() == 0 {
        return Err("Empty string".into());
    }
    if name.starts_with("#") {
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
    } else {
        let srgb = named::from_str(name).ok_or("No such color in palette")?;
        Ok(Color::Rgb {
            r: srgb.red,
            g: srgb.green,
            b: srgb.blue,
        })
    }
}

// For succinctness, if we are confident that the name will be known,
// this routine is available to keep the code readable
fn _from_known(name: &str) -> Color {
    from_string(name).unwrap()
}

// Boil down a meaning-color hashmap into a theme, by taking the defaults
// for any unknown colors
fn make_theme(overrides: &HashMap<Meaning, Color>) -> Theme {
    let colors = HashMap::from([
        (Meaning::AlertError, Color::Red),
        (Meaning::AlertWarning, Color::Yellow),
        (Meaning::AlertInfo, Color::Green),
        (Meaning::Annotation, Color::DarkGrey),
        (Meaning::Guidance, Color::Blue),
        (Meaning::Important, Color::White),
        (Meaning::Base, Color::Grey),
    ])
    .iter()
    .map(|(name, color)| match overrides.get(name) {
        Some(value) => (*name, *value),
        None => (*name, *color),
    })
    .collect();
    Theme::new(colors)
}

// Built-in themes. Rather than having extra files added before any theming
// is available, this gives a couple of basic options, demonstrating the use
// of themes: autumn and marine
lazy_static! {
    static ref ALERT_TYPES: HashMap<Level, Meaning> = {
        HashMap::from([
            (Level::Info, Meaning::AlertInfo),
            (Level::Warning, Meaning::AlertWarning),
            (Level::Error, Meaning::AlertError),
        ])
    };
    static ref BUILTIN_THEMES: HashMap<&'static str, Theme> = {
        HashMap::from([
            ("", HashMap::new()),
            (
                "autumn",
                HashMap::from([
                    (Meaning::AlertError, _from_known("saddlebrown")),
                    (Meaning::AlertWarning, _from_known("darkorange")),
                    (Meaning::AlertInfo, _from_known("gold")),
                    (Meaning::Annotation, Color::DarkGrey),
                    (Meaning::Guidance, _from_known("brown")),
                ]),
            ),
            (
                "marine",
                HashMap::from([
                    (Meaning::AlertError, _from_known("yellowgreen")),
                    (Meaning::AlertWarning, _from_known("cyan")),
                    (Meaning::AlertInfo, _from_known("turquoise")),
                    (Meaning::Annotation, _from_known("steelblue")),
                    (Meaning::Base, _from_known("lightsteelblue")),
                    (Meaning::Guidance, _from_known("teal")),
                ]),
            ),
        ])
        .iter()
        .map(|(name, theme)| (*name, make_theme(theme)))
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
    pub fn load_theme_from_file(&mut self, name: &str) -> Result<&Theme, Box<dyn error::Error>> {
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
        let colors: HashMap<Meaning, String> = config
            .try_deserialize()
            .map_err(|e| println!("failed to deserialize: {}", e))
            .unwrap();
        let theme = Theme::from_map(colors, self.debug);
        let name = name.to_string();
        self.loaded_themes.insert(name.clone(), theme);
        let theme = self.loaded_themes.get(&name).unwrap();
        Ok(theme)
    }

    // Check if the requested theme is loaded and, if not, then attempt to get it
    // from the builtins or, if not there, from file
    pub fn load_theme(&mut self, name: &str) -> &Theme {
        if self.loaded_themes.contains_key(name) {
            return self.loaded_themes.get(name).unwrap();
        }
        let built_ins = &BUILTIN_THEMES;
        match built_ins.get(name) {
            Some(theme) => theme,
            None => match self.load_theme_from_file(name) {
                Ok(theme) => theme,
                Err(err) => {
                    println!["Could not load theme {}: {}", name, err];
                    built_ins.get("").unwrap()
                }
            },
        }
    }
}

#[cfg(test)]
mod theme_tests {
    use super::*;

    #[test]
    fn load_theme() {
        let mut manager = ThemeManager::new(Some(false), Some("".to_string()));
        let theme = manager.load_theme("autumn");
        assert_eq!(
            theme.as_style(Meaning::Guidance).foreground_color,
            from_string("brown").ok()
        );
    }
}