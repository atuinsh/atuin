use atuin_client::settings::Settings;
use clap::{Args, Subcommand, ValueEnum};
use eyre::Result;
use toml_edit::{Document, DocumentMut, Item, Table, TableLike, Value};
use tracing::instrument;

#[cfg(feature = "daemon")]
use crate::command::client::daemon;
use crate::i18n::fl;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    #[command(about = fl!("cmd-config-get"))]
    Get(GetCmd),

    #[command(about = fl!("cmd-config-set"))]
    Set(SetCmd),

    #[command(about = fl!("cmd-config-enable"))]
    Enable(EnableCmd),

    #[command(about = fl!("cmd-config-print"), long_about = fl!("cmd-config-print", "long"))]
    Print(PrintCmd),
}

impl Cmd {
    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(self, settings: &Settings) -> Result<()> {
        match self {
            Self::Get(get) => get.run(settings).await,
            Self::Set(set) => set.run(settings).await,
            Self::Enable(enable) => enable.run(settings).await,
            Self::Print(print) => print.run(settings).await,
        }
    }
}

/// Get a configuration value from your config.toml file,
/// or optionally the effective value after defaults and overrides are applied.
#[derive(Args, Debug)]
pub struct GetCmd {
    #[arg(help = fl!("arg-config-get-key"))]
    pub key: String,

    #[arg(long, short, help = fl!("arg-config-get-resolved"))]
    pub resolved: bool,

    #[arg(long, short, help = fl!("arg-config-get-verbose"))]
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
            Self::print_effective_value(key, "  ").await;
            return Ok(());
        }

        if self.resolved {
            Self::print_effective_value(key, "").await;
        } else {
            self.print_current_value(key, "").await?;
        }

        Ok(())
    }

    async fn print_current_value(&self, key: &str, prefix: &str) -> Result<()> {
        let config_file = Settings::get_config_path().await?;
        let config_str = tokio::fs::read_to_string(&config_file).await?;
        let doc = config_str.parse::<Document<_>>()?;

        let current = get_deep_key(&doc, key);

        match current {
            Some(item) if item.is_table() || item.is_inline_table() => {
                let table =
                    item.as_table_like().expect("is_table()/is_inline_table() but no table");
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

    async fn print_effective_value(key: &str, prefix: &str) {
        match Settings::get_config_value(key).await {
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
    #[arg(help = fl!("arg-config-set-key"))]
    pub key: String,

    #[arg(help = fl!("arg-config-set-value"))]
    pub value: String,

    #[arg(
        long = "type",
        short,
        value_enum,
        default_value_t = ValueType::Auto,
        value_name = "TYPE",
        help = fl!("arg-config-set-the-type")
    )]
    pub the_type: ValueType,
}

#[derive(ValueEnum, Debug, Clone, PartialEq, Eq)]
pub enum ValueType {
    #[value(help = fl!("value-config-set-the-type-auto"))]
    Auto,
    #[value(help = fl!("value-config-set-the-type-string"))]
    String,
    #[value(help = fl!("value-config-set-the-type-boolean"))]
    Boolean,
    #[value(help = fl!("value-config-set-the-type-integer"))]
    Integer,
    #[value(help = fl!("value-config-set-the-type-float"))]
    Float,
}

impl SetCmd {
    pub async fn run(self, _settings: &Settings) -> Result<()> {
        let config_file = Settings::get_config_path().await?;
        let config_str = tokio::fs::read_to_string(&config_file).await?;

        let updated = self.get_updated_config(&config_str)?;
        tokio::fs::write(&config_file, &updated).await?;
        Ok(())
    }

    fn get_updated_config(&self, config_str: &str) -> Result<String> {
        let key = self.key.trim();
        if key.is_empty() || key.contains(char::is_whitespace) {
            eyre::bail!("Config key must be non-empty and must not contain whitespace");
        }

        let mut doc: DocumentMut = config_str.parse()?;

        // When using auto type detection, try to match the existing value's type
        // so we don't accidentally change e.g. "300" (string) to 300 (integer)
        let existing_type = detect_existing_type(&doc, key);
        let value = self.parse_value(existing_type.as_ref())?;
        set_deep_key(&mut doc, key, value)?;

        let updated = doc.to_string();
        Settings::validate_str(&updated).map_err(|e| {
            eyre::eyre!(
                "cannot update config: setting '{key}' to '{}' would make your configuration \
                 invalid\n\n{e}",
                self.value,
            )
        })?;
        Ok(updated)
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
                let b: bool =
                    raw.parse().map_err(|_| eyre::eyre!("invalid boolean value: {raw}"))?;
                Ok(Value::from(b))
            }
            ValueType::Integer => {
                let i: i64 =
                    raw.parse().map_err(|_| eyre::eyre!("invalid integer value: {raw}"))?;
                Ok(Value::from(i))
            }
            ValueType::Float => {
                let f: f64 = raw.parse().map_err(|_| eyre::eyre!("invalid float value: {raw}"))?;
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
pub struct EnableCmd {
    #[arg(value_enum, help = fl!("arg-config-enable-feature"))]
    pub feature: Feature,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    #[value(help = fl!("value-config-enable-feature-daemon"))]
    Daemon,
    #[value(help = fl!("value-config-enable-feature-output-capture"))]
    OutputCapture,
}

impl EnableCmd {
    pub async fn run(self, settings: &Settings) -> Result<()> {
        let config_file = Settings::get_config_path().await?;
        let config_str = tokio::fs::read_to_string(&config_file).await?;

        let updated = self.get_updated_config(&config_str, settings.daemon.enabled)?;
        tokio::fs::write(&config_file, &updated).await?;

        println!("Enabled.");

        // The daemon reads these settings only at startup, so it needs a restart to pick them
        // up. A running daemon with autostart off may be externally managed (systemd, launchd),
        // so leave that one alone and tell the user instead.
        #[cfg(feature = "daemon")]
        if settings.daemon.enabled && !settings.daemon.autostart {
            println!("Restart the Atuin daemon and your shell for the change to take effect.");
            return Ok(());
        } else if let Err(e) = daemon::restart_cmd(settings).await {
            eprintln!(
                "Could not restart the Atuin daemon: {e}\nRun `atuin daemon restart` manually."
            );
        }

        println!("Restart your shell for the change to take effect.");

        Ok(())
    }

    fn get_updated_config(&self, config_str: &str, daemon_enabled: bool) -> Result<String> {
        let mut doc: DocumentMut = config_str.parse()?;

        // An already-running daemon may be managed externally (systemd, launchd), so leave
        // its autostart alone.
        if !daemon_enabled {
            set_deep_key(&mut doc, "daemon.enabled", Value::from(true))?;
            set_deep_key(&mut doc, "daemon.autostart", Value::from(true))?;
        }

        match self.feature {
            Feature::Daemon => set_deep_key(&mut doc, "search_mode", Value::from("daemon-fuzzy"))?,
            Feature::OutputCapture => {
                set_deep_key(&mut doc, "pty_proxy.enabled", Value::from(true))?;
                set_deep_key(&mut doc, "output.enabled", Value::from(true))?;
            }
        }

        let updated = doc.to_string();
        Settings::validate_str(&updated)
            .map_err(|e| eyre::eyre!("cannot update config: it would be invalid\n\n{e}"))?;

        Ok(updated)
    }
}

#[derive(Args, Debug)]
pub struct PrintCmd {
    #[arg(help = fl!("arg-config-print-key"))]
    pub key: Option<String>,
}

impl PrintCmd {
    pub async fn run(&self, _settings: &Settings) -> Result<()> {
        let config_file = Settings::get_config_path().await?;
        let config_str = tokio::fs::read_to_string(&config_file).await?;
        let doc = config_str.parse::<Document<_>>()?;

        if let Some(key) = &self.key {
            let current = get_deep_key(&doc, key);

            if let Some(current) = current {
                if current.is_table() || current.is_inline_table() {
                    println!("[{key}]");
                    dump_table(
                        current.as_table_like().expect("is_table()/is_inline_table() but no table"),
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

            let table = value.as_table_like().expect("is_table()/is_inline_table() but no table");

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
        current = current.and_then(|item| item.as_table_like()).and_then(|table| table.get(part));
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

    if let Some(item) = current.get_mut(last) {
        let mut value = value;
        if let Some(old_value) = item.as_value_mut() {
            // Preserve any commands attached to the old value.
            std::mem::swap(value.decor_mut(), old_value.decor_mut());
        }
        *item = Item::Value(value);
    } else {
        current.insert(last, Item::Value(value));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Call [`set_deep_key`] and reserialize.
    fn set(input: &str, key: &str, value: Value) -> String {
        let mut doc: DocumentMut = input.parse().expect("test input should parse as TOML");
        set_deep_key(&mut doc, key, value).expect("set_deep_key should succeed");
        doc.to_string()
    }

    #[rstest]
    #[case::comment_above_the_key(
        "[sync]\n# how often to sync\nfrequency = \"5m\"\nrecords = true\n",
        "sync.frequency",
        Value::from("10m"),
        "[sync]\n# how often to sync\nfrequency = \"10m\"\nrecords = true\n"
    )]
    #[case::multiline_comment_and_blank_lines_above_the_key(
        "# top of file\n\n[sync]\n\n# line one\n# line two\nfrequency = \"5m\"\nrecords = true\n",
        "sync.frequency",
        Value::from("10m"),
        "# top of file\n\n[sync]\n\n# line one\n# line two\nfrequency = \"10m\"\nrecords = true\n"
    )]
    #[case::comment_above_a_root_level_key(
        "# why we sync\nauto_sync = true\n\n[sync]\nrecords = true\n",
        "auto_sync",
        Value::from(false),
        "# why we sync\nauto_sync = false\n\n[sync]\nrecords = true\n"
    )]
    #[case::trailing_comment_on_the_value(
        "[sync]\nfrequency = \"5m\" # sync interval\n",
        "sync.frequency",
        Value::from("10m"),
        "[sync]\nfrequency = \"10m\" # sync interval\n"
    )]
    #[case::compact_spacing_around_equals(
        "[sync]\nfrequency=\"5m\"\n",
        "sync.frequency",
        Value::from("10m"),
        "[sync]\nfrequency=\"10m\"\n"
    )]
    #[case::comments_on_a_dotted_key(
        "# note about records\nsync.records = true # enabled\n",
        "sync.records",
        Value::from(false),
        "# note about records\nsync.records = false # enabled\n"
    )]
    #[case::comments_around_an_inline_table(
        "# above\nkeys = { scroll_exits = true } # after\n",
        "keys.scroll_exits",
        Value::from(false),
        "# above\nkeys = { scroll_exits = false } # after\n"
    )]
    fn set_preserves_formatting_when_overwriting(
        #[case] input: &str,
        #[case] key: &str,
        #[case] value: Value,
        #[case] expected: &str,
    ) {
        assert_eq!(set(input, key, value), expected);
    }

    #[rstest]
    #[case::appends_without_disturbing_an_existing_comment(
        "[sync]\n# existing note\nrecords = true\n",
        "sync.frequency",
        Value::from("10m"),
        "[sync]\n# existing note\nrecords = true\nfrequency = \"10m\"\n"
    )]
    #[case::creates_a_missing_table(
        "[sync]\nrecords = true\n",
        "keys.scroll_exits",
        Value::from(true),
        "[sync]\nrecords = true\n\n[keys]\nscroll_exits = true\n"
    )]
    #[case::lands_above_a_commented_out_block_in_a_table(
        "[sync]\n## how often\n# frequency = \"5m\"\n",
        "sync.frequency",
        Value::from("10m"),
        "[sync]\nfrequency = \"10m\"\n## how often\n# frequency = \"5m\"\n"
    )]
    #[case::root_key_leaves_commented_out_settings_alone(
        "# atuin config\n\n## enable sync\n# auto_sync = true\n\nenter_accept = true\n",
        "auto_sync",
        Value::from(false),
        "# atuin config\n\n## enable sync\n# auto_sync = true\n\nenter_accept = true\nauto_sync = \
         false\n"
    )]
    fn set_adds_a_missing_key_without_touching_existing_content(
        #[case] input: &str,
        #[case] key: &str,
        #[case] value: Value,
        #[case] expected: &str,
    ) {
        assert_eq!(set(input, key, value), expected);
    }

    #[rstest]
    fn setting_the_same_key_twice_keeps_its_comment() {
        let once = set(
            "[sync]\n# how often to sync\nfrequency = \"5m\" # unit is flexible\n",
            "sync.frequency",
            Value::from("10m"),
        );
        let twice = set(&once, "sync.frequency", Value::from("30m"));
        assert_eq!(twice, "[sync]\n# how often to sync\nfrequency = \"30m\" # unit is flexible\n");
    }

    /// Helper for building a [`SetCmd`].
    fn set_cmd(key: &str, value: &str) -> SetCmd {
        SetCmd {
            key: key.to_string(),
            value: value.to_string(),
            the_type: ValueType::Auto,
        }
    }

    #[rstest]
    #[case::a_valid_value(
        "search_mode = \"fuzzy\"\n",
        "search_mode",
        "prefix",
        "search_mode = \"prefix\"\n"
    )]
    #[case::replacing_an_invalid_value_with_a_valid_one(
        "search_mode = \"invalid\"\n",
        "search_mode",
        "fuzzy",
        "search_mode = \"fuzzy\"\n"
    )]
    #[case::preserving_comments_and_unrelated_keys(
        "# my config\nauto_sync = true\n\n[daemon]\nenabled = false\n",
        "daemon.enabled",
        "true",
        "# my config\nauto_sync = true\n\n[daemon]\nenabled = true\n"
    )]
    fn set_writes(
        #[case] input: &str,
        #[case] key: &str,
        #[case] value: &str,
        #[case] expected: &str,
    ) {
        let updated =
            set_cmd(key, value).get_updated_config(input).expect("the update should be accepted");

        assert_eq!(updated, expected);
    }

    #[rstest]
    #[case::daemon(
        Feature::Daemon,
        "",
        false,
        "search_mode = \"daemon-fuzzy\"\n\n[daemon]\nenabled = true\nautostart = true\n"
    )]
    #[case::output_capture(
        Feature::OutputCapture,
        "",
        false,
        "[daemon]\nenabled = true\nautostart = true\n\n[pty_proxy]\nenabled = \
         true\n\n[output]\nenabled = true\n"
    )]
    #[case::daemon_already_enabled(
        Feature::Daemon,
        "[daemon]\nenabled = true\nautostart = false\n",
        true,
        "search_mode = \"daemon-fuzzy\"\n[daemon]\nenabled = true\nautostart = false\n"
    )]
    #[case::output_capture_with_daemon_already_enabled(
        Feature::OutputCapture,
        "[daemon]\nenabled = true\nautostart = false\n",
        true,
        "[daemon]\nenabled = true\nautostart = false\n\n[pty_proxy]\nenabled = \
         true\n\n[output]\nenabled = true\n"
    )]
    fn enable_writes(
        #[case] feature: Feature,
        #[case] input: &str,
        #[case] daemon_enabled: bool,
        #[case] expected: &str,
    ) {
        let updated = EnableCmd { feature }
            .get_updated_config(input, daemon_enabled)
            .expect("the update should be accepted");

        assert_eq!(updated, expected);
    }

    /// The error should always mention every listed fragment.
    #[rstest]
    #[case::an_invalid_value(
        "search_mode = \"fuzzy\"\n",
        "search_mode",
        "invalid",
        &["search_mode", "invalid"]
    )]
    // auto_sync is absent, so type detection falls back to string
    #[case::a_value_of_the_wrong_type_for_a_new_key("", "auto_sync", "banana", &["auto_sync"])]
    #[case::another_key_in_the_file_being_invalid(
        "style = \"nope\"\n",
        "auto_sync",
        "false",
        &["style"]
    )]
    #[case::an_empty_key("", "  ", "fuzzy", &["non-empty"])]
    fn set_rejects(
        #[case] input: &str,
        #[case] key: &str,
        #[case] value: &str,
        #[case] expected_err: &[&str],
    ) {
        let err = set_cmd(key, value)
            .get_updated_config(input)
            .expect_err("the update should be rejected")
            .to_string();

        for fragment in expected_err {
            assert!(err.contains(fragment), "error should mention `{fragment}`, got: {err}");
        }
    }
}
