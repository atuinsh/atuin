//! Head-to-head benchmark for `Settings::build_config`.
//!
//! Every shell hook constructs `Settings`, which parses `config.toml` and scans
//! the environment. The old `build_config` did this three times per call (a
//! `data_dir`-only pre-parse, a `build_cloned` to read paths, then a final
//! `build` with path overrides); the new one materializes the layered config
//! once and expands paths in place.
//!
//! Both paths are replicated here (the real ones are private) over an identical,
//! realistic `config.toml`, so the comparison is robust to machine load. `main`
//! asserts they resolve to the same paths before benchmarking.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use config::builder::DefaultState;
use config::{Config, ConfigBuilder, Environment, File as ConfigFile, FileFormat};
use serde::Deserialize;
use tempfile::TempDir;

const PATH_KEYS: [&str; 8] = [
    "db_path",
    "record_store_path",
    "key_path",
    "daemon.socket_path",
    "daemon.pidfile_path",
    "logs.dir",
    "logs.search.file",
    "logs.daemon.file",
];

/// A realistic ~30-line user config. Deliberately has no `data_dir`, the common
/// case, so the new path materializes the layered config exactly once.
const CONFIG_TOML: &str = r#"
auto_sync = true
sync_frequency = "5m"
sync_address = "https://api.atuin.sh"
search_mode = "fuzzy"
filter_mode = "global"
style = "compact"
inline_height = 25
show_preview = true
max_preview_height = 4
show_help = true
enter_accept = true
keymap_mode = "vim-normal"
history_filter = ["^secret", "password", "^ "]
cwd_filter = ["^/tmp", "/private"]
secrets_filter = true
strip_trailing_whitespace = true
word_jump_mode = "emacs"

[keys]
scroll_exits = false
prefix = "a"

[daemon]
enabled = true
sync_frequency = 300

[preview]
strategy = "static"

[search]
recency_score_multiplier = 2.0
filters = ["global", "host", "session", "directory"]
"#;

/// Replicates the private `Settings::builder_with_data_dir`: ~90 defaults seeded
/// from `data_dir`, plus the environment source. Faithful in shape and cost;
/// private constants are inlined as literals.
fn builder_with_data_dir(data_dir: &Path) -> ConfigBuilder<DefaultState> {
    let s = |p: PathBuf| p.to_str().unwrap().to_owned();
    let db_path = s(data_dir.join("history.db"));
    let record_store_path = s(data_dir.join("records.db"));
    let kv_path = s(data_dir.join("kv.db"));
    let scripts_path = s(data_dir.join("scripts.db"));
    let ai_sessions_path = s(data_dir.join("ai_sessions.db"));
    let pidfile_path = s(data_dir.join("atuin-daemon.pid"));
    let key_path = s(data_dir.join("key"));
    let meta_path = s(data_dir.join("meta.db"));
    let logs_dir = s(data_dir.join("logs"));

    Config::builder()
        .set_default("history_format", "{time}\t{command}\t{duration}")
        .unwrap()
        .set_default("db_path", db_path)
        .unwrap()
        .set_default("record_store_path", record_store_path)
        .unwrap()
        .set_default("key_path", key_path)
        .unwrap()
        .set_default("dialect", "us")
        .unwrap()
        .set_default("timezone", "local")
        .unwrap()
        .set_default("auto_sync", true)
        .unwrap()
        .set_default("update_check", false)
        .unwrap()
        .set_default("update_channel", "stable")
        .unwrap()
        .set_default("sync_address", "https://api.atuin.sh")
        .unwrap()
        .set_default("sync_frequency", "5m")
        .unwrap()
        .set_default("search_mode", "fuzzy")
        .unwrap()
        .set_default("filter_mode", None::<String>)
        .unwrap()
        .set_default("style", "compact")
        .unwrap()
        .set_default("inline_height", 40)
        .unwrap()
        .set_default("show_preview", true)
        .unwrap()
        .set_default("preview.strategy", "auto")
        .unwrap()
        .set_default("max_preview_height", 4)
        .unwrap()
        .set_default("show_help", true)
        .unwrap()
        .set_default("show_tabs", true)
        .unwrap()
        .set_default("show_numeric_shortcuts", true)
        .unwrap()
        .set_default("auto_hide_height", 8)
        .unwrap()
        .set_default("invert", false)
        .unwrap()
        .set_default("exit_mode", "return-original")
        .unwrap()
        .set_default("word_jump_mode", "emacs")
        .unwrap()
        .set_default(
            "word_chars",
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
        )
        .unwrap()
        .set_default("scroll_context_lines", 1)
        .unwrap()
        .set_default("shell_up_key_binding", false)
        .unwrap()
        .set_default("workspaces", false)
        .unwrap()
        .set_default("ctrl_n_shortcuts", false)
        .unwrap()
        .set_default("secrets_filter", true)
        .unwrap()
        .set_default("strip_trailing_whitespace", true)
        .unwrap()
        .set_default("network_connect_timeout", 5)
        .unwrap()
        .set_default("network_timeout", 30)
        .unwrap()
        .set_default("extra_headers", std::collections::HashMap::<String, String>::new())
        .unwrap()
        .set_default("local_timeout", 2.0)
        .unwrap()
        .set_default("enter_accept", false)
        .unwrap()
        .set_default("keys.scroll_exits", true)
        .unwrap()
        .set_default("keys.accept_past_line_end", true)
        .unwrap()
        .set_default("keys.exit_past_line_start", true)
        .unwrap()
        .set_default("keys.accept_past_line_start", false)
        .unwrap()
        .set_default("keys.accept_with_backspace", false)
        .unwrap()
        .set_default("keys.prefix", "a")
        .unwrap()
        .set_default("keymap_mode", "emacs")
        .unwrap()
        .set_default("keymap_mode_shell", "auto")
        .unwrap()
        .set_default("keymap_cursor", std::collections::HashMap::<String, String>::new())
        .unwrap()
        .set_default("smart_sort", false)
        .unwrap()
        .set_default("command_chaining", false)
        .unwrap()
        .set_default("store_failed", true)
        .unwrap()
        .set_default("daemon.sync_frequency", 300)
        .unwrap()
        .set_default("daemon.enabled", false)
        .unwrap()
        .set_default("daemon.autostart", false)
        .unwrap()
        .set_default("daemon.socket_path", None::<String>)
        .unwrap()
        .set_default("daemon.pidfile_path", pidfile_path)
        .unwrap()
        .set_default("daemon.systemd_socket", false)
        .unwrap()
        .set_default("daemon.tcp_port", 8889)
        .unwrap()
        .set_default("output.enabled", false)
        .unwrap()
        .set_default("output.max_output_size", "1048576")
        .unwrap()
        .set_default("output.sync", false)
        .unwrap()
        .set_default("output.max_disk_usage", "1073741824")
        .unwrap()
        .set_default("logs.enabled", true)
        .unwrap()
        .set_default("logs.dir", logs_dir)
        .unwrap()
        .set_default("logs.level", "info")
        .unwrap()
        .set_default("logs.search.file", "search.log")
        .unwrap()
        .set_default("logs.daemon.file", "daemon.log")
        .unwrap()
        .set_default("logs.ai.file", "ai.log")
        .unwrap()
        .set_default("kv.db_path", kv_path)
        .unwrap()
        .set_default("scripts.db_path", scripts_path)
        .unwrap()
        .set_default("search.recency_score_multiplier", 1.0)
        .unwrap()
        .set_default("search.frequency_score_multiplier", 1.0)
        .unwrap()
        .set_default("search.frecency_score_multiplier", 1.0)
        .unwrap()
        .set_default("search.shells", "auto")
        .unwrap()
        .set_default("meta.db_path", meta_path)
        .unwrap()
        .set_default("ai.db_path", ai_sessions_path)
        .unwrap()
        .set_default("ai.session_continue_minutes", 60)
        .unwrap()
        .set_default("ai.send_cwd", false)
        .unwrap()
        .set_default("ai.opening.send_cwd", false)
        .unwrap()
        .set_default("ai.opening.send_last_command", false)
        .unwrap()
        .set_default("ui.syntax_highlight", true)
        .unwrap()
        .set_default("search.filters", vec![
            "global",
            "host",
            "session",
            "workspace",
            "directory",
            "session-preload",
        ])
        .unwrap()
        .set_default("theme.name", "default")
        .unwrap()
        .set_default("theme.debug", None::<bool>)
        .unwrap()
        .set_default("tmux.enabled", false)
        .unwrap()
        .set_default("tmux.width", "80%")
        .unwrap()
        .set_default("tmux.height", "60%")
        .unwrap()
        .set_default("prefers_reduced_motion", false)
        .unwrap()
        .set_default("no_mouse", false)
        .unwrap()
        .set_default("pty_proxy.enabled", false)
        .unwrap()
        .add_source(Environment::with_prefix("atuin").prefix_separator("_").separator("__"))
}

fn expand(path: &str) -> String {
    shellexpand::full(path).map(|p| p.to_string()).unwrap_or_else(|_| path.to_owned())
}

// --- OLD: three full materializations (data_dir pre-parse + build_cloned + build) ---

#[derive(Deserialize, Default)]
struct DataDirOnly {
    data_dir: Option<String>,
}

fn old_build_config(config_file: &Path, default_dir: &Path) -> Config {
    let config_file_str = config_file.to_str().unwrap();

    let partial = Config::builder()
        .add_source(ConfigFile::new(config_file_str, FileFormat::Toml))
        .add_source(Environment::with_prefix("atuin").prefix_separator("_").separator("__"))
        .build()
        .ok();
    let custom = partial.and_then(|c| c.try_deserialize::<DataDirOnly>().ok()).and_then(|d| d.data_dir);
    let effective = match custom {
        Some(dir) => PathBuf::from(shellexpand::full(&dir).unwrap().into_owned()),
        None => default_dir.to_path_buf(),
    };

    let mut builder =
        builder_with_data_dir(&effective).add_source(ConfigFile::new(config_file_str, FileFormat::Toml));

    let built = builder.build_cloned().unwrap();
    builder = PATH_KEYS
        .iter()
        .map(|key| (*key, built.get_string(key).unwrap_or_default()))
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| (key, expand(&value)))
        .fold(builder, |b, (key, value)| b.set_override(key, value).unwrap());

    builder.build().unwrap()
}

// --- NEW: one materialization (+ rebuild only for a custom data_dir), in-place expand ---

fn build_layered(data_dir: &Path, config_file: Option<&str>) -> Config {
    let mut builder = builder_with_data_dir(data_dir);
    if let Some(path) = config_file {
        builder = builder.add_source(ConfigFile::new(path, FileFormat::Toml));
    }
    builder.build().unwrap()
}

fn configured_data_dir(config: &Config) -> Option<String> {
    std::env::var("ATUIN_DATA_DIR").ok().or_else(|| config.get_string("data_dir").ok())
}

fn set_config_string(config: &mut Config, key: &str, value: String) {
    use config::ValueKind;

    let segments: Vec<&str> = key.split('.').collect();
    let mut current = &mut config.cache;
    for (i, segment) in segments.iter().enumerate() {
        let ValueKind::Table(map) = &mut current.kind else {
            return;
        };
        let Some(next) = map.get_mut(*segment) else {
            return;
        };
        if i == segments.len() - 1 {
            next.kind = ValueKind::String(value);
            return;
        }
        current = next;
    }
}

fn new_build_config(config_file: &Path, default_dir: &Path) -> Config {
    let config_file_str = config_file.to_str().unwrap();

    let mut config = build_layered(default_dir, Some(config_file_str));

    let effective = match configured_data_dir(&config) {
        Some(dir) => PathBuf::from(shellexpand::full(&dir).unwrap().into_owned()),
        None => default_dir.to_path_buf(),
    };
    if effective != default_dir {
        config = build_layered(&effective, Some(config_file_str));
    }

    for key in PATH_KEYS {
        let Ok(value) = config.get_string(key) else {
            continue;
        };
        if value.is_empty() {
            continue;
        }
        let expanded = expand(&value);
        if expanded != value {
            set_config_string(&mut config, key, expanded);
        }
    }

    config
}

// --- shared, untimed fixture ---

struct Fixture {
    _dir: TempDir,
    config_file: PathBuf,
    data_dir: PathBuf,
}

static FIXTURE: LazyLock<Fixture> = LazyLock::new(|| {
    let dir = tempfile::tempdir().unwrap();
    let config_file = dir.path().join("config.toml");
    std::fs::write(&config_file, CONFIG_TOML).unwrap();
    let data_dir = dir.path().join("data");
    Fixture { _dir: dir, config_file, data_dir }
});

fn inputs() -> (PathBuf, PathBuf) {
    (FIXTURE.config_file.clone(), FIXTURE.data_dir.clone())
}

#[divan::bench(min_time = 1)]
fn old(bencher: divan::Bencher) {
    bencher.with_inputs(inputs).bench_values(|(config_file, data_dir)| {
        divan::black_box(old_build_config(&config_file, &data_dir))
    });
}

#[divan::bench(min_time = 1)]
fn new(bencher: divan::Bencher) {
    bencher.with_inputs(inputs).bench_values(|(config_file, data_dir)| {
        divan::black_box(new_build_config(&config_file, &data_dir))
    });
}

fn main() {
    // The two paths must resolve identically, or the comparison is meaningless.
    let (config_file, data_dir) = inputs();
    let old = old_build_config(&config_file, &data_dir);
    let new = new_build_config(&config_file, &data_dir);
    for key in PATH_KEYS {
        assert_eq!(
            old.get_string(key).ok(),
            new.get_string(key).ok(),
            "old and new disagree on `{key}`"
        );
    }
    assert_eq!(old.get_string("sync_address").ok(), new.get_string("sync_address").ok());
    assert_eq!(old.get_bool("daemon.enabled").ok(), new.get_bool("daemon.enabled").ok());

    divan::main();
}
