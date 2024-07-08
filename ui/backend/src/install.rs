// Handle installing the Atuin CLI
// We can use the standard install script for this

use std::process::Command;

use tokio::{
    fs::{read_to_string, OpenOptions},
    io::AsyncWriteExt,
};

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
pub(crate) async fn is_cli_installed() -> Result<bool, String> {
    let shell = Shell::default_shell().map_err(|e| format!("Failed to get default shell: {e}"))?;
    let output = if shell == Shell::Powershell {
        shell
            .run_interactive(&["atuin --version; if ($?) {echo 'ATUIN FOUND'}"])
            .map_err(|e| format!("Failed to run interactive command"))?
    } else {
        shell
            .run_interactive(&["atuin --version && echo 'ATUIN FOUND'"])
            .map_err(|e| format!("Failed to run interactive command"))?
    };

    Ok(output.contains("ATUIN FOUND"))
}

#[tauri::command]
pub(crate) async fn setup_cli() -> Result<(), String> {
    let shell = Shell::default_shell().map_err(|e| format!("Failed to get default shell: {e}"))?;
    let config_file_path = shell.config_file();

    if config_file_path.is_none() {
        return Err("Failed to fetch default config file".to_string());
    }

    let config_file_path = config_file_path.unwrap();
    let config_file = read_to_string(config_file_path.clone())
        .await
        .map_err(|e| format!("Failed to read config file: {e}"))?;

    if config_file.contains("atuin init") {
        return Ok(());
    }

    let mut file = OpenOptions::new()
        .write(true)
        .append(true)
        .open(config_file_path)
        .await
        .unwrap();

    let config = format!(
        "if [ -x \"$(command -v atuin)\" ]; then eval \"$(atuin init {})\"; fi",
        shell.to_string()
    );
    file.write_all(config.as_bytes())
        .await
        .map_err(|e| format!("Failed to write Atuin shell init: {e}"));

    Ok(())
}
