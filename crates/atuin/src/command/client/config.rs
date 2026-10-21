use std::sync::Arc;

#[cfg(feature = "octavo")]
use atuin_client::history::HistoryId;
use atuin_client::settings::Settings;
use atuin_common::string::StrExt;
#[cfg(feature = "octavo")]
use atuin_domain::caps::OctavoCap;
#[cfg(feature = "octavo")]
use atuin_octavo::hub::{HubCallError, HubClient, UserId};
#[cfg(feature = "octavo")]
use atuin_octavo::pb;
#[cfg(feature = "octavo")]
use atuin_octavo::queue::UploadQueue;
use clap::{Args, Subcommand, ValueEnum};
use eyre::Result;
#[cfg(feature = "octavo")]
use eyre::WrapErr;
use toml_edit::{Document, DocumentMut, Item, Table, TableLike, Value};
use tracing::instrument;

#[cfg(all(feature = "ai", feature = "daemon"))]
use crate::command::client::ai_sessions;
#[cfg(feature = "daemon")]
use crate::command::client::daemon;
use crate::i18n::fl;

mod path;
use path::TomlPath;

mod comment;
use comment::enable_example;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    #[command(about = fl!("cmd-config-get"))]
    Get(GetCmd),

    #[command(about = fl!("cmd-config-set"))]
    Set(SetCmd),

    #[command(about = fl!("cmd-config-enable"))]
    Enable(EnableCmd),

    #[command(about = fl!("cmd-config-disable"))]
    Disable(DisableCmd),

    #[command(about = fl!("cmd-config-print"), long_about = fl!("cmd-config-print", "long"))]
    Print(PrintCmd),
}

impl Cmd {
    #[instrument(level = "trace", skip_all)]
    pub async fn run(self, settings: &Arc<Settings>) -> Result<()> {
        match self {
            Self::Get(get) => get.run(settings).await,
            Self::Set(set) => set.run(settings).await,
            Self::Enable(enable) => enable.run(settings).await,
            Self::Disable(disable) => disable.run(settings).await,
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
    pub async fn run(&self, settings: &Settings) -> Result<()> {
        let key = self.key.trim();
        if key.is_empty() || key.contains(char::is_whitespace) {
            eyre::bail!("Config key must be non-empty and must not contain whitespace");
        }

        if self.verbose {
            println!("Config file:");
            self.print_current_value(key, "  ").await?;
            println!("\nResolved:");
            Self::print_effective_value(settings, key, "  ");
            return Ok(());
        }

        if self.resolved {
            Self::print_effective_value(settings, key, "");
        } else {
            self.print_current_value(key, "").await?;
        }

        Ok(())
    }

    async fn print_current_value(&self, key: &str, prefix: &str) -> Result<()> {
        let config_file = Settings::get_config_path()?;
        let config_str = tokio::fs::read_to_string(&config_file).await?;
        let doc = config_str.parse::<Document<_>>()?;

        let current = get_deep_key(doc.as_item(), key);

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

    fn print_effective_value(settings: &Settings, key: &str, prefix: &str) {
        match settings.get_config_value(key) {
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
        let config_file = Settings::get_config_path()?;
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

        let doc: DocumentMut = config_str.parse()?;

        // When using auto type detection, try to match the existing value's type
        // so we don't accidentally change e.g. "300" (string) to 300 (integer)
        let existing_type = detect_existing_type(&doc, key);
        let value = self.parse_value(existing_type.as_ref())?;
        let updated = update_config(config_str, [(key, value)])?;

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

#[derive(Args, Debug)]
pub struct DisableCmd {
    #[arg(value_enum, help = fl!("arg-config-disable-feature"))]
    pub feature: Feature,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    #[value(help = fl!("value-config-feature-daemon"))]
    Daemon,
    #[value(help = fl!("value-config-feature-output-capture"))]
    OutputCapture,
    #[cfg(feature = "octavo")]
    #[value(help = fl!("value-config-feature-octavo"))]
    Octavo,
    #[cfg(all(feature = "ai", feature = "daemon"))]
    #[value(help = fl!("value-config-feature-threads"), alias = "ai-sessions")]
    Threads,
}

impl EnableCmd {
    pub async fn run(self, settings: &Arc<Settings>) -> Result<()> {
        #[cfg(all(feature = "ai", feature = "daemon"))]
        if self.feature == Feature::Threads {
            ai_sessions::enable(settings, &mut ai_sessions::Terminal::stdio()).await?;
            return Ok(());
        }

        #[cfg(feature = "octavo")]
        let octavo = match self.feature {
            Feature::Octavo => {
                let Some(onboarding) = onboard_octavo(settings).await? else {
                    println!("{}", fl!("config-enable-octavo-declined"));
                    return Ok(());
                };
                Some(onboarding)
            }
            _ => None,
        };

        let config_file = Settings::get_config_path()?;
        let config_str = tokio::fs::read_to_string(&config_file).await?;

        let updated = self.get_updated_config(&config_str, settings.daemon.enabled)?;
        tokio::fs::write(&config_file, &updated).await?;

        println!("Enabled.");

        // Queued before the restart below so the daemon starts on it, and reported after it so a
        // failure still leaves the daemon on the new config.
        #[cfg(feature = "octavo")]
        let backfilled = match octavo {
            Some(onboarding) => onboarding.backfill().await,
            None => Ok(()),
        };
        #[cfg(not(feature = "octavo"))]
        let backfilled = Ok(());

        // The daemon reads these settings only at startup, so it needs a restart to pick them
        // up. A running daemon with autostart off may be externally managed (systemd, launchd),
        // so leave that one alone and tell the user instead.
        #[cfg(feature = "daemon")]
        if settings.daemon.enabled && !settings.daemon.autostart {
            println!("Restart the Atuin daemon and your shell for the change to take effect.");
            return backfilled;
        } else if let Err(e) = daemon::restart_cmd(settings, false).await {
            eprintln!(
                "Could not restart the Atuin daemon: {e}\nRun `atuin daemon restart` manually."
            );
        }

        println!("Restart your shell for the change to take effect.");

        backfilled
    }

    pub(super) fn get_updated_config(
        &self,
        config_str: &str,
        daemon_enabled: bool,
    ) -> Result<String> {
        let mut changes = Vec::new();

        // An already-running daemon may be managed externally (systemd, launchd), so leave
        // its autostart alone.
        if !daemon_enabled {
            changes.push(("daemon.enabled", Value::from(true)));
            changes.push(("daemon.autostart", Value::from(true)));
        }

        match self.feature {
            Feature::Daemon => changes.push(("search_mode", Value::from("daemon-fuzzy"))),
            Feature::OutputCapture => {
                changes.push(("pty_proxy.enabled", Value::from(true)));
                changes.push(("output.enabled", Value::from(true)));
            }
            #[cfg(feature = "octavo")]
            Feature::Octavo => changes.push(("octavo.enabled", Value::from(true))),
            #[cfg(all(feature = "ai", feature = "daemon"))]
            Feature::Threads => changes.push(("threads.capture", Value::from(true))),
        }

        let updated = update_config(config_str, changes)?;
        Settings::validate_str(&updated)
            .map_err(|e| eyre::eyre!("cannot update config: it would be invalid\n\n{e}"))?;

        Ok(updated)
    }
}

/// What enabling Octavo queues once the config says it's on.
#[cfg(feature = "octavo")]
struct OctavoOnboarding {
    queue: UploadQueue,
    user: UserId,
    backfill: Vec<HistoryId>,
    old: usize,
}

#[cfg(feature = "octavo")]
impl OctavoOnboarding {
    async fn backfill(self) -> Result<()> {
        if !self.backfill.is_empty() {
            let queued = self.queue.push_many_history(&self.user, self.backfill).await.wrap_err(
                "failed to queue your history for upload; run `atuin config enable octavo` again \
                 to retry",
            )?;
            println!("{}", fl!("config-enable-octavo-backfilled", count = queued));
        }
        if self.old > 0 {
            println!("{}", fl!("config-enable-octavo-backfill-old", count = self.old));
        }

        Ok(())
    }
}

/// Asks the user to enable Octavo, and whether to upload the history they already have; `None`
/// if they decline.
#[cfg(feature = "octavo")]
async fn onboard_octavo(settings: &Arc<Settings>) -> Result<Option<OctavoOnboarding>> {
    let login = match HubClient::new(settings)?.login(settings).await {
        Ok(login) => login,
        Err(HubCallError::NotLoggedIn) => eyre::bail!(fl!("config-enable-octavo-not-logged-in")),
        Err(err) => return Err(err).wrap_err("failed to ask the hub who you are logged in as"),
    };

    let caps = atuin_client::api_client::caps_client(settings)?;
    caps.refresh().await?;
    if caps.get_server::<OctavoCap>().await?.is_none() {
        eyre::bail!(fl!(
            "config-enable-octavo-not-enabled",
            address = settings.sync_address.to_string()
        ));
    }

    let accepted = super::setup::prompt(
        "Octavo",
        &fl!("config-enable-octavo-description"),
        Some(&fl!("config-enable-octavo-disclaimer")),
        super::setup::DefaultAnswer::No,
    )?;
    if !accepted {
        return Ok(None);
    }

    let history =
        atuin_client::database::Sqlite::new(&settings.db_path, settings.local_timeout).await?;
    let (ids, old): (Vec<_>, Vec<_>) =
        history.active_ids().await?.into_iter().partition(|&id| pb::UuidV7::try_from(id).is_ok());

    let backfill = super::setup::prompt(
        "history backfill",
        &fl!("config-enable-octavo-backfill", count = ids.len()),
        None,
        super::setup::DefaultAnswer::No,
    )?;

    let (backfill, old) = if backfill {
        (ids, old.len())
    } else {
        (Vec::new(), 0)
    };

    // Opened before the config says Octavo is on, so a queue that can't be opened leaves it off.
    let queue = UploadQueue::open(settings.octavo_queue_path())
        .await
        .wrap_err("failed to open Octavo's upload queue")?;

    Ok(Some(OctavoOnboarding {
        queue,
        user: login.user_id().clone(),
        backfill,
        old,
    }))
}

impl DisableCmd {
    pub async fn run(self, settings: &Settings) -> Result<()> {
        let config_file = Settings::get_config_path()?;
        let config_str = tokio::fs::read_to_string(&config_file).await?;

        let updated = self.get_updated_config(&config_str)?;
        tokio::fs::write(&config_file, &updated).await?;

        println!("Disabled.");

        #[cfg(feature = "daemon")]
        match self.feature {
            Feature::Daemon => {
                if let Err(e) = daemon::stop_cmd(settings).await {
                    eprintln!(
                        "Could not stop the Atuin daemon: {e}\nRun `atuin daemon stop` manually."
                    );
                    return Ok(());
                }
            }
            Feature::OutputCapture => restart_autostarted_daemon(settings).await,
            #[cfg(feature = "octavo")]
            Feature::Octavo => restart_autostarted_daemon(settings).await,
            // The daemon notices the change itself, and the shell doesn't care.
            #[cfg(feature = "ai")]
            Feature::Threads => return Ok(()),
        }

        println!("Restart your shell for the change to take effect.");

        Ok(())
    }

    fn get_updated_config(&self, config_str: &str) -> Result<String> {
        let doc: DocumentMut = config_str.parse()?;
        let mut changes = Vec::new();

        match self.feature {
            Feature::Daemon => {
                changes.push(("daemon.enabled", Value::from(false)));
                changes.push(("daemon.autostart", Value::from(false)));
                if doc.get("search_mode").and_then(Item::as_str) == Some("daemon-fuzzy") {
                    changes.push(("search_mode", Value::from("fuzzy")));
                }
            }
            Feature::OutputCapture => changes.push(("output.enabled", Value::from(false))),
            #[cfg(feature = "octavo")]
            Feature::Octavo => changes.push(("octavo.enabled", Value::from(false))),
            #[cfg(all(feature = "ai", feature = "daemon"))]
            Feature::Threads => changes.push(("threads.capture", Value::from(false))),
        }

        let updated = update_config(config_str, changes)?;
        Settings::validate_str(&updated)
            .map_err(|e| eyre::eyre!("cannot update config: it would be invalid\n\n{e}"))?;

        Ok(updated)
    }
}

#[cfg(feature = "daemon")]
async fn restart_autostarted_daemon(settings: &Settings) {
    if settings.daemon.enabled
        && settings.daemon.autostart
        && let Err(e) = daemon::restart_cmd(settings, true).await
    {
        eprintln!("Could not restart the Atuin daemon: {e}\nRun `atuin daemon restart` manually.");
    }
}

#[derive(Args, Debug)]
pub struct PrintCmd {
    #[arg(help = fl!("arg-config-print-key"))]
    pub key: Option<String>,
}

impl PrintCmd {
    pub async fn run(&self, _settings: &Settings) -> Result<()> {
        let config_file = Settings::get_config_path()?;
        let config_str = tokio::fs::read_to_string(&config_file).await?;
        let doc = config_str.parse::<Document<_>>()?;

        if let Some(key) = &self.key {
            let current = get_deep_key(doc.as_item(), key);

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

fn get_deep_key<'doc>(root: &'doc Item, key: &str) -> Option<&'doc Item> {
    let parts = key.split('.');
    let mut current: Option<&Item> = Some(root);

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

/// Apply a list of changes to the config.
///
/// `changes` is a list of key-value pairs that should be set in `config`. The keys may be dotted
/// paths, as expected by [`set_deep_key`].
///
/// If `config` doesn't already contain a key, this function tries to insert it at an intelligent
/// location -- at or near a commented-out example of that key if one exists. See [`enable_example`]
/// for more information.
fn update_config<'a, C>(config: &str, changes: C) -> Result<String>
where
    // These bounds allow passing arrays or `Vec`s. Don't try changing the `AsRef`-slice bound to
    // `for<'c> &'c C: IntoIterator<Item = &'c C::Item>` for more flexibility: it causes compilation
    // to fail on macOS due to a recursive impl from the `objc2` crate. See
    // <https://github.com/rust-lang/rust/issues/136856> for more info.
    C: IntoIterator<Item = (&'a str, Value)> + AsRef<[C::Item]>,
{
    let mut config = config;
    let mut owned_config;

    for (key, _) in changes.as_ref() {
        let path = TomlPath::parse_dotted_simple(key);
        if let Some(new_config) = enable_example(config, path) {
            owned_config = new_config;
            config = &owned_config;
        }
    }

    let mut doc: DocumentMut = config.parse()?;
    for (key, value) in changes {
        set_deep_key(&mut doc, key, value)?;
    }

    Ok(doc.to_string())
}

fn set_deep_key(doc: &mut DocumentMut, key: &str, value: Value) -> Result<()> {
    let mut parts = key.split('.');
    let Some(last) = parts.next_back() else {
        eyre::bail!("empty config key");
    };
    let parent = parts;

    // Check whether setting the key will create a new table. If so, ensure it ends up at the end of
    // the file, after any trailing comments. Otherwise, `toml_edit` will likely put it above an
    // unrelated block of comments that could be example keys for a different table, thereby
    // invalidating the examples.
    let will_create_table = parent
        .clone()
        .try_fold(doc.as_item(), |item, part| item.as_table_like()?.get(part))
        .is_none();

    // Take the document's trailing comments so we can reapply them in the correct place.
    let mut trailing = if will_create_table
        && let Some(trailing) = doc.trailing().as_str()
        && !trailing.is_empty()
    {
        let ends_with_blank = trailing.lines().next_back().is_some_and(StrExt::is_space_or_empty);
        let trailing = trailing.strip_suffix('\n').unwrap_or(trailing);
        let trailing = format!(
            "{trailing}\n{}",
            if ends_with_blank {
                ""
            } else {
                "\n"
            },
        );
        doc.set_trailing("");
        Some(trailing)
    } else {
        None
    };

    let mut current: &mut dyn TableLike = doc.as_table_mut();

    // Navigate/create intermediate tables
    for part in parent {
        if !current.contains_key(part) {
            let mut table = Table::new();

            // Ensure the new table ends up at the end. Note that tables with the same position are
            // written in insertion order.
            table.set_position(Some(isize::MAX));

            // Reapply the comments from the end of the file above this table.
            if let Some(comments) = trailing.take() {
                table.decor_mut().set_prefix(comments);
            }
            current.insert(part, Item::Table(table));
        }

        current = current
            .get_mut(part)
            .expect("just inserted or already exists")
            .as_table_like_mut()
            .ok_or_else(|| eyre::eyre!("'{}' exists but is not a table", part))?;
    }

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

    /// Call [`update_config`] with a single key.
    fn set(input: &str, key: &str, value: Value) -> String {
        update_config(input, [(key, value)]).expect("the update should succeed")
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
    #[case::creates_a_missing_table_after_the_last_tables_comments(
        "[ui]\nx = 1\n## more\n# y = 2\n",
        "a.b",
        Value::from(1),
        "[ui]\nx = 1\n## more\n# y = 2\n\n[a]\nb = 1\n"
    )]
    #[case::creates_a_missing_table_after_comments_without_a_trailing_newline(
        "[ui]\n## more\n# y = 2",
        "a.b",
        Value::from(1),
        "[ui]\n## more\n# y = 2\n\n[a]\nb = 1\n"
    )]
    #[case::creates_a_missing_table_after_comments_and_a_blank_line(
        "[ui]\n## more\n# y = 2\n\n",
        "a.b",
        Value::from(1),
        "[ui]\n## more\n# y = 2\n\n[a]\nb = 1\n"
    )]
    #[case::creates_a_missing_table_after_comments_and_a_line_of_spaces(
        "[ui]\n## more\n# y = 2\n  \n",
        "a.b",
        Value::from(1),
        "[ui]\n## more\n# y = 2\n  \n[a]\nb = 1\n"
    )]
    #[case::creates_a_missing_table_after_root_comments(
        "x = 1\n## more\n# y = 2\n",
        "a.b",
        Value::from(1),
        "x = 1\n## more\n# y = 2\n\n[a]\nb = 1\n"
    )]
    #[case::creates_missing_nested_tables_after_the_comments(
        "[ui]\n## more\n# y = 2\n",
        "a.b.c",
        Value::from(1),
        "[ui]\n## more\n# y = 2\n\n[a]\n\n[a.b]\nc = 1\n"
    )]
    #[case::creates_a_missing_subtable_at_the_end_not_before_the_next_tables_comments(
        "[daemon]\nx = 1\n## more\n# y = 2\n\n[ui]\nz = 3\n",
        "daemon.sub.b",
        Value::from(1),
        "[daemon]\nx = 1\n## more\n# y = 2\n\n[ui]\nz = 3\n\n[daemon.sub]\nb = 1\n"
    )]
    #[case::lands_above_prose_comments_in_a_table(
        "[sync]\n## how often\n",
        "sync.frequency",
        Value::from("10m"),
        "[sync]\nfrequency = \"10m\"\n## how often\n"
    )]
    fn set_adds_a_missing_key_without_touching_existing_content(
        #[case] input: &str,
        #[case] key: &str,
        #[case] value: Value,
        #[case] expected: &str,
    ) {
        assert_eq!(set(input, key, value), expected);
    }

    /// The key is placed by [`comment::enable_example`], which has the tests of where; these check
    /// that its value is then set as asked.
    #[rstest]
    #[case::uncommenting_an_example(
        "[sync]\n## how often\n# frequency = \"5m\"\n",
        "sync.frequency",
        Value::from("10m"),
        "[sync]\n## how often\nfrequency = \"10m\"\n"
    )]
    #[case::uncommenting_an_example_of_another_type(
        "## how often\n# sync_frequency=\"5m\" # or 0\n",
        "sync_frequency",
        Value::from(0),
        "## how often\nsync_frequency=0 # or 0\n"
    )]
    #[case::inserting_after_an_example(
        "## filters\n# history_filter = [\n#   \"^secret\",\n# ]\n\nauto_sync = true\n",
        "history_filter",
        Value::from("fuzzy"),
        "## filters\n# history_filter = [\n#   \"^secret\",\n# ]\nhistory_filter = \
         \"fuzzy\"\n\nauto_sync = true\n"
    )]
    #[case::inserting_after_an_example_of_another_type(
        "# sync_frequency = \"5m\"\n",
        "sync_frequency",
        Value::from(true),
        "# sync_frequency = \"5m\"\nsync_frequency = true\n"
    )]
    #[case::already_set_elsewhere(
        "## how often\n# sync_frequency = \"5m\"\n\nsync_frequency = \"1h\"\n",
        "sync_frequency",
        Value::from("10m"),
        "## how often\n# sync_frequency = \"5m\"\n\nsync_frequency = \"10m\"\n"
    )]
    fn set_a_documented_key(
        #[case] input: &str,
        #[case] key: &str,
        #[case] value: Value,
        #[case] expected: &str,
    ) {
        assert_eq!(set(input, key, value), expected);
    }

    /// The motivating case: the value lands on the documented line, not at the top of the file.
    #[rstest]
    fn set_search_mode_in_the_default_config() {
        let config = Settings::example_config();
        let updated = set_cmd("search_mode", "daemon-fuzzy")
            .get_updated_config(config)
            .expect("the update should be accepted");

        let expected = config
            .replacen("# search_mode = \"fuzzy\"", "search_mode = \"daemon-fuzzy\"", 1)
            .replace("\r\n", "\n"); // toml_edit always writes LF line endings
        assert_eq!(updated, expected);
    }

    /// A new table goes after every comment in the file, which all document existing tables.
    #[rstest]
    fn set_a_key_in_a_new_table_in_the_default_config() {
        let config = Settings::example_config();
        // toml_edit always writes LF line endings.
        let expected = format!("{config}\n[a]\nb = 1\n").replace("\r\n", "\n");
        assert_eq!(set(config, "a.b", Value::from(1)), expected);
    }

    #[rstest]
    fn new_tables_from_one_update_all_go_after_the_comments() {
        let updated = update_config("[ui]\n## more\n# y = 2\n", [
            ("a.x", Value::from(1)),
            ("b.y", Value::from(2)),
        ])
        .expect("the update should succeed");

        assert_eq!(updated, "[ui]\n## more\n# y = 2\n\n[a]\nx = 1\n\n[b]\ny = 2\n");
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
    #[cfg_attr(
        all(feature = "ai", feature = "daemon"),
        case::threads(
            Feature::Threads,
            "",
            false,
            "[daemon]\nenabled = true\nautostart = true\n\n[threads]\ncapture = true\n"
        )
    )]
    #[cfg_attr(
        all(feature = "ai", feature = "daemon"),
        case::threads_with_daemon_already_enabled(
            Feature::Threads,
            "[daemon]\nenabled = true\nautostart = false\n",
            true,
            "[daemon]\nenabled = true\nautostart = false\n\n[threads]\ncapture = true\n"
        )
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

    #[rstest]
    #[case::daemon(Feature::Daemon, "", "[daemon]\nenabled = false\nautostart = false\n")]
    #[case::output_capture(Feature::OutputCapture, "", "[output]\nenabled = false\n")]
    #[cfg_attr(
        all(feature = "ai", feature = "daemon"),
        case::threads(
            Feature::Threads,
            "[daemon]\nenabled = true\n\n[threads]\ncapture = true\n",
            "[daemon]\nenabled = true\n\n[threads]\ncapture = false\n"
        )
    )]
    #[case::daemon_search(
        Feature::Daemon,
        "search_mode = \"daemon-fuzzy\"\n[daemon]\nenabled = true",
        "search_mode = \"fuzzy\"\n[daemon]\nenabled = false\nautostart = false\n"
    )]
    #[case::non_daemon_search(
        Feature::Daemon,
        "search_mode = \"prefix\"\n[daemon]\nenabled = true",
        "search_mode = \"prefix\"\n[daemon]\nenabled = false\nautostart = false\n"
    )]
    fn disable_writes(#[case] feature: Feature, #[case] input: &str, #[case] expected: &str) {
        let updated = DisableCmd { feature }
            .get_updated_config(input)
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
