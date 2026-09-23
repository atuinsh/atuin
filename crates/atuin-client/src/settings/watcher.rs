//! Config file watching for automatic settings reload.
//!
//! This module provides a `SettingsWatcher` that monitors the config file
//! for changes and broadcasts updated settings via a `tokio::sync::watch` channel.
//!
//! # Example
//!
//! ```no_run
//! use atuin_client::settings::watcher::global_settings_watcher;
//!
//! async fn example() -> eyre::Result<()> {
//!     let watcher = global_settings_watcher().await?;
//!     let mut rx = watcher.subscribe();
//!
//!     // React to settings changes
//!     while rx.changed().await.is_ok() {
//!         let settings = rx.borrow();
//!         println!("Settings updated!");
//!     }
//!     Ok(())
//! }
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use atuin_common::fs;
use eyre::{Result, WrapErr};
use notify::event::{EventKind, ModifyKind};
use notify::{Config as NotifyConfig, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{OnceCell, watch};
use tracing::{debug, error, info, warn};

use super::Settings;

/// Global singleton for the settings watcher.
static SETTINGS_WATCHER: OnceCell<Result<SettingsWatcher, String>> = OnceCell::const_new();

/// Get the global settings watcher singleton.
///
/// Initializes the watcher on first call. Subsequent calls return the same instance.
/// The watcher monitors the config file for changes and broadcasts updates.
pub async fn global_settings_watcher() -> Result<&'static SettingsWatcher> {
    let result = SETTINGS_WATCHER
        .get_or_init(async || SettingsWatcher::new().await.map_err(|e| e.to_string()))
        .await;

    match result {
        Ok(watcher) => Ok(watcher),
        Err(e) => Err(eyre::eyre!("{}", e)),
    }
}

/// Watches the config file for changes and broadcasts updated settings.
///
/// Uses `notify` for cross-platform file watching and `tokio::sync::watch`
/// for efficient broadcast to multiple subscribers.
pub struct SettingsWatcher {
    /// Receiver for settings updates. Clone this to subscribe.
    rx: watch::Receiver<Arc<Settings>>,
    /// Keeps the file watcher alive for the lifetime of this struct.
    _watcher: RecommendedWatcher,
}

impl SettingsWatcher {
    /// Create a new settings watcher.
    ///
    /// Loads initial settings and starts watching the config file for changes.
    /// Changes are debounced (500ms) to avoid multiple reloads during saves.
    pub async fn new() -> Result<Self> {
        let initial_settings = Arc::new(Settings::new().await?);
        let (tx, rx) = watch::channel(initial_settings);

        let config_path = Self::config_path();
        info!("starting config file watcher: {:?}", config_path);

        let watcher = Self::create_watcher(tx, &config_path).await?;

        Ok(Self {
            rx,
            _watcher: watcher,
        })
    }

    /// Subscribe to settings updates.
    ///
    /// Returns a receiver that will be notified when settings change.
    /// Use `changed().await` to wait for the next update, then `borrow()`
    /// to access the current settings.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<Arc<Settings>> {
        self.rx.clone()
    }

    /// Get the current settings without subscribing to updates.
    #[must_use]
    pub fn current(&self) -> Arc<Settings> {
        self.rx.borrow().clone()
    }

    /// Get the config file path.
    fn config_path() -> PathBuf {
        let config_dir = if let Ok(p) = std::env::var("ATUIN_CONFIG_DIR") {
            PathBuf::from(p)
        } else {
            atuin_common::utils::config_dir()
        };
        config_dir.join("config.toml")
    }

    /// Create the file watcher with debouncing.
    async fn create_watcher(
        tx: watch::Sender<Arc<Settings>>,
        config_path: &Path,
    ) -> Result<RecommendedWatcher> {
        // Channel for debouncing file events
        let (debounce_tx, debounce_rx) = std::sync::mpsc::channel::<()>();

        // Spawn debounce thread
        let config_path_clone = config_path.to_path_buf();
        std::thread::spawn(move || {
            Self::debounce_loop(&debounce_rx, &tx, &config_path_clone);
        });

        // Clone config_path for use in the watcher callback
        let config_path_for_watcher = config_path.to_path_buf();

        // Canonicalize config path for reliable comparison on macOS
        // (handles symlinks like /var -> /private/var)
        let canonical_config_path = fs::canonicalize(&config_path_for_watcher)
            .await
            .unwrap_or_else(|_| config_path_for_watcher.clone());

        // Create file watcher
        let mut watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                match res {
                    Ok(event) => {
                        // Defensive: if paths is empty, we can't filter, so assume
                        // it might be our config file and trigger a reload to be safe
                        if event.paths.is_empty() {
                            warn!(
                                "config watcher: event has no paths, triggering reload to be safe"
                            );
                            let _ = debounce_tx.send(());
                            return;
                        }

                        // Only react to events for our specific config file
                        // (filter out editor temp files, backups, etc.)
                        let is_config_file = event.paths.iter().any(|path| {
                            // Canonicalize for reliable comparison (handles macOS symlinks). This
                            // runs on notify's own thread, outside any runtime, so it blocks.
                            let canonical_event_path =
                                fs::blocking::canonicalize(path).unwrap_or_else(|_| path.clone());

                            // Check if this event is for our config file
                            // (either exact match or the file was renamed to our config)
                            canonical_event_path == canonical_config_path
                                || path.file_name() == config_path_for_watcher.file_name()
                        });

                        if !is_config_file {
                            return;
                        }

                        // Only react to modify events (content changes) or creates
                        if matches!(
                            event.kind,
                            EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Any)
                                | EventKind::Create(_)
                        ) {
                            debug!("config file event detected: {:?}", event);
                            // Send to debounce channel (ignore send errors - receiver might be gone)
                            let _ = debounce_tx.send(());
                        }
                    }
                    Err(e) => {
                        error!("file watcher error: {}", e);
                    }
                }
            },
            NotifyConfig::default(),
        )
        .wrap_err("failed to create file watcher")?;

        // Watch the config file's parent directory (some editors create new files)
        let watch_path = config_path.parent().unwrap_or(config_path);

        // Defensive: ensure watch path exists before trying to watch
        if !fs::exists(watch_path).await.unwrap_or(false) {
            warn!("config directory does not exist, creating it: {:?}", watch_path);
            fs::create_dir_all(watch_path)
                .await
                .wrap_err_with(|| format!("failed to create config directory: {watch_path:?}"))?;
        }

        watcher
            .watch(watch_path, RecursiveMode::NonRecursive)
            .wrap_err_with(|| format!("failed to watch config directory: {watch_path:?}"))?;

        info!("config file watcher initialized for: {:?}", watch_path);
        Ok(watcher)
    }

    /// Debounce loop that batches file events and reloads settings.
    ///
    /// Runs on its own thread, outside any runtime, so its filesystem access blocks.
    fn debounce_loop(
        rx: &std::sync::mpsc::Receiver<()>,
        tx: &watch::Sender<Arc<Settings>>,
        config_path: &Path,
    ) {
        const DEBOUNCE_DURATION: Duration = Duration::from_millis(500);

        loop {
            // Wait for first event
            if rx.recv().is_err() {
                // Channel closed, watcher was dropped
                debug!("config watcher debounce loop exiting");
                return;
            }

            // Drain any additional events within debounce window
            while rx.recv_timeout(DEBOUNCE_DURATION).is_ok() {
                // Keep draining
            }

            // Defensive: check if config file exists before reloading
            // (handles case where file was deleted - we'll get notified when it's recreated)
            if !fs::blocking::exists(config_path).unwrap_or(false) {
                debug!("config file does not exist, skipping reload: {:?}", config_path);
                continue;
            }

            // Now reload settings
            info!("config file changed, reloading settings: {:?}", config_path);
            match Settings::new_blocking() {
                Ok(settings) => {
                    if tx.send(Arc::new(settings)).is_err() {
                        // All receivers dropped
                        debug!("all settings subscribers dropped, exiting");
                        return;
                    }
                    info!("settings reloaded successfully");
                }
                Err(e) => {
                    warn!("failed to reload settings: {}", e);
                    // Keep the old settings, don't broadcast the error
                }
            }
        }
    }
}
