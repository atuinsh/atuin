use std::fmt::Write as _;
use std::process::Command;
use std::{io, process};

#[cfg(feature = "client")]
use std::path::{Path, PathBuf};

#[cfg(feature = "client")]
use atuin_client::{
    plugin::{OfficialPlugin, OfficialPluginRegistry},
    settings::Settings,
};
use clap::CommandFactory;
use clap::builder::{StyledStr, Styles};
use eyre::Result;
#[cfg(feature = "client")]
use semver::Version;

use crate::Atuin;

#[cfg(feature = "client")]
pub fn run(args: &[String], settings: Option<&Settings>) -> Result<()> {
    let Some(subcommand) = args.first() else {
        return Ok(());
    };

    let bin = format!("atuin-{subcommand}");
    let registry = OfficialPluginRegistry::new();
    let plugin = registry.get_plugin(subcommand);

    let plugins_enabled = settings.is_none_or(|cfg| cfg.plugins.enabled);
    let auto_install_enabled = settings.is_none_or(|cfg| cfg.plugins.auto_install);
    let auto_update_enabled = settings.is_none_or(|cfg| cfg.plugins.auto_update);
    let is_excluded =
        settings.is_some_and(|cfg| cfg.plugins.exclude.iter().any(|name| name == subcommand));
    let plugin_automation_enabled = plugins_enabled && !is_excluded;
    let auto_manage_compiled = cfg!(feature = "plugin-auto-manage");
    let can_manage_plugins = if auto_manage_compiled {
        can_manage_companion_plugins()
    } else {
        false
    };
    let is_companion_plugin = plugin.is_some_and(|p| p.is_companion);
    let unmanaged_companion = is_companion_plugin && auto_manage_compiled && !can_manage_plugins;
    let auto_manage_disabled = is_companion_plugin && !auto_manage_compiled;

    let mut resolved_plugin_bin = plugin.and_then(|_| registry.find_plugin_binary(subcommand));

    if resolved_plugin_bin.is_none()
        && let Some(official_plugin) = plugin
        && official_plugin.is_companion
        && plugin_automation_enabled
        && auto_install_enabled
        && can_manage_plugins
    {
        eprintln!("atuin: installing plugin '{subcommand}'...");
        match registry.install_plugin(subcommand) {
            Ok(()) => {
                write_cached_plugin_version(subcommand, env!("CARGO_PKG_VERSION"));
                resolved_plugin_bin = registry.find_plugin_binary(subcommand);
                if resolved_plugin_bin.is_none() {
                    eprintln!(
                        "atuin: installation completed but '{}' was not found in '{}' or PATH",
                        official_plugin.bin_name,
                        OfficialPluginRegistry::plugin_install_dir().display()
                    );
                }
            }
            Err(err) => {
                eprintln!("atuin: failed to install plugin '{subcommand}': {err:#}");
            }
        }
    }

    if let Some(official_plugin) = plugin
        && official_plugin.is_companion
        && plugin_automation_enabled
        && auto_update_enabled
        && can_manage_plugins
        && let Some(binary_path) = resolved_plugin_bin.clone()
    {
        maybe_update_companion_plugin(
            &registry,
            official_plugin,
            &binary_path,
            &mut resolved_plugin_bin,
        );
    }

    let mut cmd = match resolved_plugin_bin {
        Some(path) => Command::new(path),
        None => Command::new(&bin),
    };
    cmd.args(&args[1..]);

    let spawn_result = match cmd.spawn() {
        Ok(child) => Ok(child),
        Err(e) => match e.kind() {
            io::ErrorKind::NotFound => {
                let output = if auto_manage_disabled {
                    render_auto_manage_disabled_not_found(subcommand)
                } else if unmanaged_companion {
                    render_unmanaged_companion_not_found(subcommand)
                } else {
                    render_not_found(subcommand, &bin)
                };
                Err(output)
            }
            _ => Err(e.to_string().into()),
        },
    };

    match spawn_result {
        Ok(mut child) => {
            let status = child.wait()?;
            if status.success() {
                Ok(())
            } else {
                process::exit(status.code().unwrap_or(1));
            }
        }
        Err(e) => {
            eprintln!("{}", e.ansi());
            process::exit(1);
        }
    }
}

#[cfg(not(feature = "client"))]
pub fn run(args: &[String]) -> Result<()> {
    let Some(subcommand) = args.first() else {
        return Ok(());
    };

    let bin = format!("atuin-{subcommand}");
    let mut cmd = Command::new(&bin);
    cmd.args(&args[1..]);

    let spawn_result = match cmd.spawn() {
        Ok(child) => Ok(child),
        Err(e) => match e.kind() {
            io::ErrorKind::NotFound => {
                let output = render_not_found(subcommand, &bin);
                Err(output)
            }
            _ => Err(e.to_string().into()),
        },
    };

    match spawn_result {
        Ok(mut child) => {
            let status = child.wait()?;
            if status.success() {
                Ok(())
            } else {
                process::exit(status.code().unwrap_or(1));
            }
        }
        Err(e) => {
            eprintln!("{}", e.ansi());
            process::exit(1);
        }
    }
}

#[cfg(feature = "client")]
fn maybe_update_companion_plugin(
    registry: &OfficialPluginRegistry,
    plugin: &OfficialPlugin,
    binary_path: &Path,
    resolved_plugin_bin: &mut Option<PathBuf>,
) {
    let current_version = env!("CARGO_PKG_VERSION");
    if read_cached_plugin_version(&plugin.name).as_deref() == Some(current_version) {
        return;
    }

    let Some(installed_version) = read_plugin_version(binary_path) else {
        return;
    };

    if installed_version == current_version {
        write_cached_plugin_version(&plugin.name, current_version);
        return;
    }

    eprintln!("atuin: updating plugin '{}'...", plugin.name);

    if let Err(err) = registry.install_plugin(&plugin.name) {
        eprintln!("atuin: failed to update plugin '{}': {err:#}", plugin.name);
        return;
    }

    write_cached_plugin_version(&plugin.name, current_version);

    if let Some(path) = registry.find_plugin_binary(&plugin.name) {
        *resolved_plugin_bin = Some(path);
    } else {
        eprintln!(
            "atuin: update completed but '{}' was not found in '{}' or PATH",
            plugin.bin_name,
            OfficialPluginRegistry::plugin_install_dir().display()
        );
    }
}

#[cfg(feature = "client")]
fn read_plugin_version(binary_path: &Path) -> Option<String> {
    let output = Command::new(binary_path).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    parse_version_output(&stdout)
}

#[cfg(feature = "client")]
fn parse_version_output(version_output: &str) -> Option<String> {
    for token in version_output.split_whitespace() {
        let token = token.trim();
        let token = token.strip_prefix('v').unwrap_or(token);
        if Version::parse(token).is_ok() {
            return Some(token.to_string());
        }
    }

    None
}

#[cfg(feature = "client")]
fn plugin_version_cache_path(plugin_name: &str) -> PathBuf {
    atuin_common::utils::data_dir()
        .join("plugin-versions")
        .join(plugin_name)
}

#[cfg(feature = "client")]
fn read_cached_plugin_version(plugin_name: &str) -> Option<String> {
    let path = plugin_version_cache_path(plugin_name);
    let cached = fs_err::read_to_string(path).ok()?;
    let cached = cached.trim();
    if cached.is_empty() {
        None
    } else {
        Some(cached.to_string())
    }
}

#[cfg(feature = "client")]
fn write_cached_plugin_version(plugin_name: &str, version: &str) {
    let path = plugin_version_cache_path(plugin_name);
    if let Some(parent) = path.parent() {
        let _ = fs_err::create_dir_all(parent);
    }
    let _ = fs_err::write(path, format!("{version}\n"));
}

#[cfg(feature = "client")]
fn can_manage_companion_plugins() -> bool {
    let install_dir = OfficialPluginRegistry::plugin_install_dir();
    let install_dir = fs_err::canonicalize(&install_dir).unwrap_or(install_dir);

    let Ok(current_exe) = std::env::current_exe() else {
        return false;
    };
    let current_exe = fs_err::canonicalize(&current_exe).unwrap_or(current_exe);

    current_exe.starts_with(install_dir)
}

#[cfg(feature = "client")]
fn render_auto_manage_disabled_not_found(subcommand: &str) -> StyledStr {
    let mut output = StyledStr::new();
    let styles = Styles::styled();
    let error = styles.get_error();
    let invalid = styles.get_invalid();

    let _ = write!(output, "{error}error:{error:#} ");
    let _ = write!(
        output,
        "'{invalid}{subcommand}{invalid:#}' is an official atuin plugin, but it's not installed",
    );
    let _ = write!(output, "\n\n");
    let _ = write!(
        output,
        "This atuin build has companion plugin auto-install disabled.",
    );
    let _ = write!(output, "\n");
    let _ = write!(
        output,
        "Install 'atuin-{subcommand}' with your package manager, or install atuin via script if you want auto-install support:\n  curl --proto '=https' --tlsv1.2 -LsSf https://setup.atuin.sh | sh",
    );

    output
}

#[cfg(feature = "client")]
fn render_unmanaged_companion_not_found(subcommand: &str) -> StyledStr {
    let mut output = StyledStr::new();
    let styles = Styles::styled();
    let error = styles.get_error();
    let invalid = styles.get_invalid();

    let _ = write!(output, "{error}error:{error:#} ");
    let _ = write!(
        output,
        "'{invalid}{subcommand}{invalid:#}' is an official atuin plugin, but it's not installed",
    );
    let _ = write!(output, "\n\n");
    let _ = write!(
        output,
        "Atuin can only auto-install companion plugins when atuin itself is managed in '~/.atuin/bin'.",
    );
    let _ = write!(output, "\n");
    let _ = write!(
        output,
        "Install atuin with the script to enable that flow:\n  curl --proto '=https' --tlsv1.2 -LsSf https://setup.atuin.sh | sh",
    );
    let _ = write!(output, "\n\n");
    let _ = write!(
        output,
        "Or install the companion binary with your package manager and ensure 'atuin-{subcommand}' is in PATH.",
    );

    output
}

fn render_not_found(subcommand: &str, bin: &str) -> StyledStr {
    let mut output = StyledStr::new();
    let styles = Styles::styled();

    let error = styles.get_error();
    let invalid = styles.get_invalid();
    let literal = styles.get_literal();

    #[cfg(feature = "client")]
    {
        let registry = OfficialPluginRegistry::new();

        if let Some(install_message) = registry.get_install_message(subcommand) {
            let _ = write!(output, "{error}error:{error:#} ");
            let _ = write!(
                output,
                "'{invalid}{subcommand}{invalid:#}' is an official atuin plugin, but it's not installed"
            );
            let _ = write!(output, "\n\n");
            let _ = write!(output, "{install_message}");
            return output;
        }
    }

    let mut atuin_cmd = Atuin::command();
    let usage = atuin_cmd.render_usage();

    let _ = write!(output, "{error}error:{error:#} ");
    let _ = write!(
        output,
        "unrecognized subcommand '{invalid}{subcommand}{invalid:#}' "
    );
    let _ = write!(
        output,
        "and no executable named '{invalid}{bin}{invalid:#}' found in your PATH"
    );
    let _ = write!(output, "\n\n");
    let _ = write!(output, "{usage}");
    let _ = write!(output, "\n\n");
    let _ = write!(
        output,
        "For more information, try '{literal}--help{literal:#}'."
    );

    output
}
