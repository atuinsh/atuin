use crate::settings;
use atuin_common::logs::{FileConfig, LogConfig};
use std::path::PathBuf;

pub trait FromSettings: Sized {
    type Output;
    fn from_settings(settings: &settings::Logs, child: &settings::LogConfig) -> Self::Output;
}

impl FromSettings for FileConfig {
    type Output = Option<Self>;

    fn from_settings(settings: &settings::Logs, child: &settings::LogConfig) -> Option<Self> {
        if !child.enabled.unwrap_or(settings.enabled) {
            return None;
        }
        Some(Self {
            path: PathBuf::from_iter([&settings.dir, &child.file]),
            level: child.level.unwrap_or(settings.level),
            retention_days: child.retention.unwrap_or(settings.retention),
        })
    }
}

impl FromSettings for LogConfig {
    type Output = Self;

    fn from_settings(settings: &settings::Logs, child: &settings::LogConfig) -> Self {
        Self {
            file: FileConfig::from_settings(settings, child),
            stderr: None,
        }
    }
}
