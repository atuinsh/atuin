use atuin_client::settings::Settings;
use clap::{Args, Subcommand, ValueEnum};
use eyre::Result;
use toml_edit::{Document, DocumentMut, Item, Table, TableLike, Value};

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Get a configuration value from your config.toml file
    /// or after defaults and overrides are applied
    #[command()]
    Get(GetCmd),

    /// Set a configuration value in your config.toml file
    #[command()]
    Set(SetCmd),

    /// Print all configuration values from your config.toml file
    /// in TOML format
    ///
    /// If a key is provided, only print the value of that key and all its children
    #[command()]
    Print(PrintCmd),
}

impl Cmd {
    pub async fn run(self, settings: &Settings) -> Result<()> {
        match self {
            Self::Get(get) => get.run(settings).await,
            Self::Set(set) => set.run(settings).await,
            Self::Print(print) => print.run(settings).await,
        }
    }
}

/// Get a configuration value from your config.toml file,
/// or optionally the effective value after defaults and overrides are applied.
#[derive(Args, Debug)]
pub struct GetCmd {
    /// The configuration key to get
    pub key: String,

    /// Print the value after defaults and overrides are applied
    #[arg(long, short)]
    pub resolved: bool,

    /// Print both the config file value and the resolved value
    #[arg(long, short)]
    pub verbose: bool,
}

impl GetCmd {
    pub async fn run(&self, _settings: &Settings) -> Result<()> {
        let key = self.key.trim();
        if key.is_empty() || key.contains(char::is_whitespace) {
            eyre::bail!("Config key must be non-empty and must not contain whitespace");
        }

        if self.verbose {
            println!("Config file:");
            self.print_current_value(key, "  ").await?;
            println!("\nResolved:");
            Self::print_effective_value(key, "  ");
            return Ok(());
        }

        if self.resolved {
            Self::print_effective_value(key, "");
        } else {
            self.print_current_value(key, "").await?;
        }

        Ok(())
    }

    async fn print_current_value(&self, key: &str, prefix: &str) -> Result<()> {
        let config_file = Settings::get_config_path()?;
        let config_str = tokio::fs::read_to_string(&config_file).await?;
        let doc = config_str.parse::<Document<_>>()?;

        let current = get_deep_key(&doc, key);

        match current {
            Some(item) if item.is_table() || item.is_inline_table() => {
                let table = item
                    .as_table_like()
                    .expect("is_table()/is_inline_table() but no table");
                println!("{prefix}[{key}]");
                dump_table(table, prefix, &mut vec![key.to_string()])?;
            }
            Some(item) => {
                let val = item.to_string();
                let val = val.trim().trim_matches('"');
                println!("{prefix}{val}");
            }
            None => {
                println!("{prefix}(not set in config file)");
            }
        }

        Ok(())
    }

    fn print_effective_value(key: &str, prefix: &str) {
        match Settings::get_config_value(key) {
            Ok(value) => {
                for line in value.lines() {
                    println!("{prefix}{line}");
                }
            }
            Err(_) => {
                println!("{prefix}(unknown key)");
            }
        }
    }
}

#[derive(Args, Debug)]
pub struct SetCmd {
    /// The configuration key to set
    pub key: String,

    /// The value to set
    pub value: String,

    /// Store value as an explicit type
    #[arg(long = "type", short, value_enum, default_value_t = ValueType::Auto, value_name = "TYPE")]
    pub the_type: ValueType,
}

#[derive(ValueEnum, Debug, Clone, PartialEq, Eq)]
pub enum ValueType {
    /// Automatically determine the type of the value
    Auto,
    /// Store value as a string
    String,
    /// Store value as a boolean
    Boolean,
    /// Store value as an integer
    Integer,
    /// Store the value as a float
    Float,
}

impl SetCmd {
    pub async fn run(self, _settings: &Settings) -> Result<()> {
        let key = self.key.trim();
        if key.is_empty() || key.contains(char::is_whitespace) {
            eyre::bail!("Config key must be non-empty and must not contain whitespace");
        }

        let config_file = Settings::get_config_path()?;
        let config_str = tokio::fs::read_to_string(&config_file).await?;
        let mut doc: DocumentMut = config_str.parse()?;

        // When using auto type detection, try to match the existing value's type
        // so we don't accidentally change e.g. "300" (string) to 300 (integer)
        let existing_type = detect_existing_type(&doc, key);
        let value = self.parse_value(existing_type.as_ref())?;
        set_deep_key(&mut doc, key, value)?;

        tokio::fs::write(&config_file, doc.to_string()).await?;

        Ok(())
    }

    fn parse_value(&self, existing_type: Option<&ValueType>) -> Result<Value> {
        let raw = &self.value;

        // Explicit --type takes priority, then existing value type, then auto-detect
        let effective_type = if self.the_type != ValueType::Auto {
            &self.the_type
        } else if let Some(existing) = existing_type {
            existing
        } else {
            &ValueType::Auto
        };

        match effective_type {
            ValueType::String => Ok(Value::from(raw.as_str())),
            ValueType::Boolean => {
                let b: bool = raw
                    .parse()
                    .map_err(|_| eyre::eyre!("invalid boolean value: {raw}"))?;
                Ok(Value::from(b))
            }
            ValueType::Integer => {
                let i: i64 = raw
                    .parse()
                    .map_err(|_| eyre::eyre!("invalid integer value: {raw}"))?;
                Ok(Value::from(i))
            }
            ValueType::Float => {
                let f: f64 = raw
                    .parse()
                    .map_err(|_| eyre::eyre!("invalid float value: {raw}"))?;
                Ok(Value::from(f))
            }
            ValueType::Auto => {
                if raw == "true" || raw == "false" {
                    return Ok(Value::from(raw == "true"));
                }
                if let Ok(i) = raw.parse::<i64>() {
                    return Ok(Value::from(i));
                }
                if let Ok(f) = raw.parse::<f64>() {
                    return Ok(Value::from(f));
                }
                Ok(Value::from(raw.as_str()))
            }
        }
    }
}

#[derive(Args, Debug)]
pub struct PrintCmd {
    /// Print the value of a specific key and all its children
    pub key: Option<String>,
}

impl PrintCmd {
    pub async fn run(&self, _settings: &Settings) -> Result<()> {
        let config_file = Settings::get_config_path()?;
        let config_str = tokio::fs::read_to_string(&config_file).await?;
        let doc = config_str.parse::<Document<_>>()?;

        if let Some(key) = &self.key {
            let current = get_deep_key(&doc, key);

            if let Some(current) = current {
                if current.is_table() || current.is_inline_table() {
                    println!("[{key}]");
                    dump_table(
                        current
                            .as_table_like()
                            .expect("is_table()/is_inline_table() but no table"),
                        "",
                        &mut vec![key.clone()],
                    )?;
                } else {
                    println!("{}", current.to_string().trim().trim_matches('"'));
                }
            } else {
                println!("key not found");
            }
        } else {
            dump_table(doc.as_table(), "", &mut Vec::new())?;
        }

        Ok(())
    }
}

fn dump_table(table: &dyn TableLike, prefix: &str, stack: &mut Vec<String>) -> Result<()> {
    for (key, value) in table.iter() {
        if value.is_table() || value.is_inline_table() {
            stack.push(key.to_string());

            let table = value
                .as_table_like()
                .expect("is_table()/is_inline_table() but no table");

            println!("\n{}[{}]", prefix, stack.join("."));

            dump_table(table, prefix, stack)?;

            stack.pop();
        } else {
            println!("{prefix}{key} = {value}");
        }
    }

    Ok(())
}

fn get_deep_key<'doc>(doc: &'doc Document<String>, key: &str) -> Option<&'doc Item> {
    let parts = key.split('.');
    let mut current: Option<&Item> = Some(doc.as_item());

    for part in parts {
        current = current
            .and_then(|item| item.as_table_like())
            .and_then(|table| table.get(part));
    }

    current
}

/// Detect the TOML type of an existing key in the document, so `set` with auto
/// type detection preserves the original type rather than guessing from the value string.
fn detect_existing_type(doc: &DocumentMut, key: &str) -> Option<ValueType> {
    let parts: Vec<&str> = key.split('.').collect();
    let mut current: &dyn TableLike = doc.as_table();

    for &part in &parts[..parts.len().saturating_sub(1)] {
        current = current.get(part)?.as_table_like()?;
    }

    let last = parts.last()?;
    let v = current.get(last)?.as_value()?;

    if v.is_str() {
        Some(ValueType::String)
    } else if v.is_bool() {
        Some(ValueType::Boolean)
    } else if v.is_integer() {
        Some(ValueType::Integer)
    } else if v.is_float() {
        Some(ValueType::Float)
    } else {
        None
    }
}

fn set_deep_key(doc: &mut DocumentMut, key: &str, value: Value) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();

    if parts.is_empty() {
        eyre::bail!("empty config key");
    }

    let mut current: &mut dyn TableLike = doc.as_table_mut();

    // Navigate/create intermediate tables
    for &part in &parts[..parts.len() - 1] {
        if !current.contains_key(part) {
            current.insert(part, Item::Table(Table::new()));
        }
        current = current
            .get_mut(part)
            .expect("just inserted or already exists")
            .as_table_like_mut()
            .ok_or_else(|| eyre::eyre!("'{}' exists but is not a table", part))?;
    }

    let last = *parts.last().unwrap();

    // Don't silently overwrite a table with a scalar value
    if let Some(existing) = current.get(last)
        && (existing.is_table() || existing.is_inline_table())
    {
        eyre::bail!(
            "'{}' is a table; use a dotted key like '{}.key' to set a value within it",
            key,
            key
        );
    }

    current.insert(last, Item::Value(value));

    Ok(())
}
