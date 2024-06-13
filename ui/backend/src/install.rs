// Handle installing the Atuin CLI
// We can use the standard install script for this

use std::process::Command;

use atuin_common::shell::Shell;

#[tauri::command]
pub(crate) async fn install_cli() -> Result<(), String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg("curl --proto '=https' --tlsv1.2 -LsSf https://github.com/atuinsh/atuin/releases/latest/download/atuin-installer.sh | sh")
        .output().map_err(|e|format!("Failed to execute Atuin installer: {e}"));

    Ok(())
}

#[tauri::command]
pub(crate) async fn is_cli_installed() -> Result<bool, String>{
    let shell = Shell::default_shell().map_err(|e|format!("Failed to get default shell: {e}"))?;
    let output = shell.run_interactive(&["atuin --version && echo 'ATUIN FOUND'"]).map_err(|e|format!("Failed to run interactive command"))?;

    Ok(output.contains("ATUIN FOUND"))
}

#[tauri::command]
pub(crate) async fn setup_cli() -> Result<(), String>{
    let shell = Shell::default_shell().map_err(|e|format!("Failed to get default shell: {e}"))?;
    let config_file = shell.config_file();

    if config_file.is_none() {
        return Err("Failed to fetch default config file".to_string());
    }

    let config_file = config_file.unwrap();

    Ok(())
}
