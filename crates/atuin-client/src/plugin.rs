use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
};

use axoupdater::{AxoUpdater, ReleaseSource, ReleaseSourceType, UpdateRequest, Version};
use eyre::{Result, WrapErr, eyre};
use fs_err::create_dir_all;

#[derive(Debug, Clone)]
pub struct OfficialPlugin {
    pub name: String,
    pub description: String,
    pub install_message: String,
    pub bin_name: String,
    pub has_init: bool,
    pub init_args: Vec<String>,
    pub supported_shells: Option<Vec<String>>,
    pub is_companion: bool,
}

impl OfficialPlugin {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: &str,
        description: &str,
        install_message: &str,
        bin_name: &str,
        has_init: bool,
        init_args: Vec<&str>,
        supported_shells: Option<Vec<&str>>,
        is_companion: bool,
    ) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            install_message: install_message.to_string(),
            bin_name: bin_name.to_string(),
            has_init,
            init_args: init_args.into_iter().map(str::to_string).collect(),
            supported_shells: supported_shells
                .map(|shells| shells.into_iter().map(str::to_string).collect()),
            is_companion,
        }
    }
}

pub struct OfficialPluginRegistry {
    plugins: HashMap<String, OfficialPlugin>,
}

impl OfficialPluginRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            plugins: HashMap::new(),
        };

        registry.register_official_plugins();

        registry
    }

    pub fn plugin_install_dir() -> PathBuf {
        atuin_common::utils::home_dir().join(".atuin").join("bin")
    }

    fn register_official_plugins(&mut self) {
        self.plugins.insert(
            "update".to_string(),
            OfficialPlugin::new(
                "update",
                "Update atuin to the latest version",
                "The 'atuin update' command is provided by the atuin-update plugin.\n\
                 It is only installed if you used the install script\n  \
                 If you used a package manager (brew, apt, etc), please continue to use it for updates",
                "atuin-update",
                false,
                vec![],
                None,
                false,
            ),
        );

        self.plugins.insert(
            "ai".to_string(),
            OfficialPlugin::new(
                "ai",
                "AI-powered shell completion",
                "The 'atuin ai' command is provided by the atuin-ai companion plugin.",
                "atuin-ai",
                true,
                vec!["init"],
                Some(vec!["zsh"]),
                true,
            ),
        );
    }

    pub fn get_plugin(&self, name: &str) -> Option<&OfficialPlugin> {
        self.plugins.get(name)
    }

    pub fn is_official_plugin(&self, name: &str) -> bool {
        self.plugins.contains_key(name)
    }

    pub fn get_install_message(&self, name: &str) -> Option<&str> {
        self.plugins
            .get(name)
            .map(|plugin| plugin.install_message.as_str())
    }

    pub fn companion_plugins(&self) -> Vec<&OfficialPlugin> {
        let mut plugins = self
            .plugins
            .values()
            .filter(|plugin| plugin.is_companion)
            .collect::<Vec<_>>();
        plugins.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        plugins
    }

    pub fn plugins_with_init(&self, shell: &str) -> Vec<&OfficialPlugin> {
        let mut plugins = self
            .plugins
            .values()
            .filter(|plugin| plugin.has_init)
            .filter(|plugin| {
                plugin
                    .supported_shells
                    .as_ref()
                    .is_none_or(|shells| shells.iter().any(|supported| supported == shell))
            })
            .collect::<Vec<_>>();
        plugins.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        plugins
    }

    pub fn find_plugin_binary(&self, name: &str) -> Option<PathBuf> {
        let plugin = self.get_plugin(name)?;
        let install_dir = Self::plugin_install_dir();

        find_binary_in_dir(&install_dir, &plugin.bin_name)
            .or_else(|| find_binary_in_path(&plugin.bin_name))
    }

    pub fn install_plugin(&self, name: &str) -> Result<()> {
        let plugin = self
            .get_plugin(name)
            .ok_or_else(|| eyre!("unknown official plugin '{name}'"))?;

        let install_dir = Self::plugin_install_dir();
        create_dir_all(&install_dir).wrap_err_with(|| {
            format!(
                "failed to create plugin install directory {}",
                install_dir.display()
            )
        })?;

        let mut updater = AxoUpdater::new_for(&plugin.bin_name);
        updater.set_release_source(ReleaseSource {
            release_type: ReleaseSourceType::GitHub,
            owner: "atuinsh".to_string(),
            name: "atuin".to_string(),
            app_name: plugin.bin_name.clone(),
        });
        if let Some(token) = env::var("ATUIN_GITHUB_TOKEN")
            .ok()
            .or_else(|| env::var("GITHUB_TOKEN").ok())
            .or_else(|| env::var("GH_TOKEN").ok())
            .filter(|token| !token.trim().is_empty())
        {
            updater.set_github_token(&token);
        }

        updater.set_install_dir(install_dir.to_string_lossy().to_string());
        updater.configure_version_specifier(UpdateRequest::SpecificVersion(
            env!("CARGO_PKG_VERSION").to_string(),
        ));
        updater.set_current_version(Version::parse("0.0.0")?)?;
        updater.always_update(true);
        updater.run_sync().wrap_err_with(|| {
            format!(
                "failed to install/update companion binary '{}'",
                plugin.bin_name
            )
        })?;

        Ok(())
    }
}

fn find_binary_in_path(bin_name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path).find_map(|dir| find_binary_in_dir(&dir, bin_name))
}

fn find_binary_in_dir(dir: &Path, bin_name: &str) -> Option<PathBuf> {
    for candidate in binary_name_candidates(bin_name) {
        let path = dir.join(candidate);
        if path.is_file() {
            return Some(path);
        }
    }

    None
}

fn binary_name_candidates(bin_name: &str) -> Vec<String> {
    let mut candidates = vec![bin_name.to_string()];
    if cfg!(windows) && !bin_name.ends_with(".exe") {
        candidates.push(format!("{bin_name}.exe"));
    }

    candidates
}

impl Default for OfficialPluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_creation() {
        let registry = OfficialPluginRegistry::new();
        assert!(registry.is_official_plugin("update"));
        assert!(registry.is_official_plugin("ai"));
        assert!(!registry.is_official_plugin("nonexistent"));
    }

    #[test]
    fn test_get_plugin() {
        let registry = OfficialPluginRegistry::new();
        let plugin = registry.get_plugin("update");
        assert!(plugin.is_some());
        assert_eq!(plugin.expect("missing plugin").name, "update");
    }

    #[test]
    fn test_get_install_message() {
        let registry = OfficialPluginRegistry::new();
        let message = registry.get_install_message("update");
        assert!(message.is_some());
        assert!(message.expect("missing message").contains("atuin-update"));
    }

    #[test]
    fn test_companion_plugins() {
        let registry = OfficialPluginRegistry::new();
        let companion_plugins = registry.companion_plugins();
        assert_eq!(companion_plugins.len(), 1);
        assert_eq!(companion_plugins[0].name, "ai");
    }

    #[test]
    fn test_plugins_with_init() {
        let registry = OfficialPluginRegistry::new();
        let zsh_plugins = registry.plugins_with_init("zsh");
        assert_eq!(zsh_plugins.len(), 1);
        assert_eq!(zsh_plugins[0].name, "ai");

        let bash_plugins = registry.plugins_with_init("bash");
        assert!(bash_plugins.is_empty());
    }
}
