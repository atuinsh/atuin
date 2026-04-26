use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct OfficialPlugin {
    pub name: String,
    pub description: String,
    pub install_message: String,
}

impl OfficialPlugin {
    pub fn new(name: &str, description: &str, install_message: &str) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            install_message: install_message.to_string(),
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

        // Register official plugins
        registry.register_official_plugins();

        registry
    }

    fn register_official_plugins(&mut self) {
        // atuin-update plugin
        self.plugins.insert(
            "update".to_string(),
            OfficialPlugin::new(
                "update",
                "Update atuin to the latest version",
                "The 'atuin update' command is provided by the atuin-update plugin.\n\
                 It is only installed if you used the install script\n  \
                 If you used a package manager (brew, apt, etc), please continue to use it for updates",
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
}

impl Default for OfficialPluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct PluginContext {
    #[cfg(windows)]
    _update_on_windows: Option<UpdateOnWindowsContext>,
}

impl PluginContext {
    pub fn new(_subcommand: &str) -> Self {
        PluginContext {
            #[cfg(windows)]
            _update_on_windows: (_subcommand == "update").then(UpdateOnWindowsContext::new),
        }
    }
}

impl Drop for PluginContext {
    fn drop(&mut self) {}
}

#[cfg(windows)]
struct UpdateOnWindowsContext {
    initial_exe: Option<std::path::PathBuf>,
}

#[cfg(windows)]
impl UpdateOnWindowsContext {
    const OLD_FILE_NAME: &'static str = "atuin.old";

    pub fn new() -> Self {
        // Windows doesn't let you overwrite a running exe, but it lets you rename it,
        // so make some room for atuin-update to install the new version.
        let initial_exe = std::env::current_exe().ok().and_then(|exe| {
            std::fs::rename(&exe, exe.with_file_name(Self::OLD_FILE_NAME)).ok()?;
            Some(exe)
        });

        Self { initial_exe }
    }
}

#[cfg(windows)]
impl Drop for UpdateOnWindowsContext {
    fn drop(&mut self) {
        if let Some(exe) = &self.initial_exe
            && !exe.exists()
        {
            // The update failed, roll back the current exe to its initial name.
            std::fs::rename(exe.with_file_name(Self::OLD_FILE_NAME), exe).unwrap_or_else(|e| {
                eprintln!("Failed to roll back the update, you may need to reinstall Atuin: {e}");
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_creation() {
        let registry = OfficialPluginRegistry::new();
        assert!(registry.is_official_plugin("update"));
        assert!(!registry.is_official_plugin("nonexistent"));
    }

    #[test]
    fn test_get_plugin() {
        let registry = OfficialPluginRegistry::new();
        let plugin = registry.get_plugin("update");
        assert!(plugin.is_some());
        assert_eq!(plugin.unwrap().name, "update");
    }

    #[test]
    fn test_get_install_message() {
        let registry = OfficialPluginRegistry::new();
        let message = registry.get_install_message("update");
        assert!(message.is_some());
        assert!(message.unwrap().contains("atuin-update"));
    }
}
