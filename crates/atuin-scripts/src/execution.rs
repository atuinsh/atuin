use crate::store::script::Script;
use eyre::Result;
use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use tempfile::NamedTempFile;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task;
use tracing::debug;

// Helper function to build a complete script with shebang
pub fn build_executable_script(script: String, shebang: String) -> String {
    if shebang.is_empty() {
        // Default to bash if no shebang is provided
        format!("#!/usr/bin/env bash\n{script}")
    } else if script.starts_with("#!") {
        format!("{shebang}\n{script}")
    } else {
        format!("#!{shebang}\n{script}")
    }
}

/// Represents the communication channels for an interactive script
pub struct ScriptSession {
    /// Channel to send input to the script
    pub stdin_tx: mpsc::Sender<String>,
    /// Exit code of the process once it completes
    pub exit_code_rx: mpsc::Receiver<i32>,
}

impl ScriptSession {
    /// Send input to the running script
    pub async fn send_input(&self, input: String) -> Result<(), mpsc::error::SendError<String>> {
        self.stdin_tx.send(input).await
    }

    /// Wait for the script to complete and get the exit code
    pub async fn wait_for_exit(&mut self) -> Option<i32> {
        self.exit_code_rx.recv().await
    }
}

fn setup_template(script: &Script) -> Result<minijinja::Environment<'_>> {
    let mut env = minijinja::Environment::new();
    env.set_trim_blocks(true);
    env.add_template("script", script.script.as_str())?;

    Ok(env)
}

/// Template a script with the given context
pub fn template_script(
    script: &Script,
    context: &HashMap<String, serde_json::Value>,
) -> Result<String> {
    let env = setup_template(script)?;
    let template = env.get_template("script")?;
    let rendered = template.render(context)?;

    Ok(rendered)
}

/// Get the variables that need to be templated in a script
pub fn template_variables(script: &Script) -> Result<HashSet<String>> {
    let env = setup_template(script)?;
    let template = env.get_template("script")?;

    Ok(template.undeclared_variables(true))
}

/// Execute a script interactively, allowing for ongoing stdin/stdout interaction
pub async fn execute_script_interactive(
    script: String,
    shebang: String,
) -> Result<ScriptSession, Box<dyn std::error::Error + Send + Sync>> {
    // Create a temporary file for the script
    let temp_file = NamedTempFile::new()?;
    let temp_path = temp_file.path().to_path_buf();

    debug!("creating temp file at {}", temp_path.display());

    // Extract interpreter from shebang for fallback execution
    let interpreter = if !shebang.is_empty() {
        shebang.trim_start_matches("#!").trim().to_string()
    } else {
        "/usr/bin/env bash".to_string()
    };

    // Write script content to the temp file, including the shebang
    let full_script_content = build_executable_script(script.clone(), shebang.clone());

    debug!("writing script content to temp file");
    tokio::fs::write(&temp_path, &full_script_content).await?;

    // Make it executable on Unix systems
    #[cfg(unix)]
    {
        debug!("making script executable");
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&temp_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&temp_path, perms)?;
    }

    // Store the temp_file to prevent it from being dropped
    // This ensures it won't be deleted while the script is running
    let _keep_temp_file = temp_file;

    debug!("attempting direct script execution");
    let mut child_result = tokio::process::Command::new(temp_path.to_str().unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    // If direct execution fails, try using the interpreter
    if let Err(e) = &child_result {
        debug!("direct execution failed: {}, trying with interpreter", e);

        // When falling back to interpreter, remove the shebang from the file
        // Some interpreters don't handle scripts with shebangs well
        debug!("writing script content without shebang for interpreter execution");
        tokio::fs::write(&temp_path, &script).await?;

        // Parse the interpreter command
        let parts: Vec<&str> = interpreter.split_whitespace().collect();
        if !parts.is_empty() {
            let mut cmd = tokio::process::Command::new(parts[0]);

            // Add any interpreter args
            for i in parts.iter().skip(1) {
                cmd.arg(i);
            }

            // Add the script path
            cmd.arg(temp_path.to_str().unwrap());

            // Try with the interpreter
            child_result = cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();
        }
    }

    // If it still fails, return the error
    let mut child = match child_result {
        Ok(child) => child,
        Err(e) => {
            return Err(format!("Failed to execute script: {e}").into());
        }
    };

    // Get handles to stdin, stdout, stderr
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Failed to open child process stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to open child process stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to open child process stderr".to_string())?;

    // Create channels for the interactive session
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(32);
    let (exit_code_tx, exit_code_rx) = mpsc::channel::<i32>(1);

    // handle user stdin
    debug!("spawning stdin handler");
    tokio::spawn(async move {
        while let Some(input) = stdin_rx.recv().await {
            if let Err(e) = stdin.write_all(input.as_bytes()).await {
                eprintln!("Error writing to stdin: {e}");
                break;
            }
            if let Err(e) = stdin.flush().await {
                eprintln!("Error flushing stdin: {e}");
                break;
            }
        }
        // when the channel closes (sender dropped), we let stdin close naturally
    });

    // handle stdout
    debug!("spawning stdout handler");
    let stdout_handle = task::spawn(async move {
        let mut stdout_reader = BufReader::new(stdout);
        let mut buffer = [0u8; 1024];
        let mut stdout_writer = tokio::io::stdout();

        loop {
            match stdout_reader.read(&mut buffer).await {
                Ok(0) => break, // End of stdout
                Ok(n) => {
                    if let Err(e) = stdout_writer.write_all(&buffer[0..n]).await {
                        eprintln!("Error writing to stdout: {e}");
                        break;
                    }
                    if let Err(e) = stdout_writer.flush().await {
                        eprintln!("Error flushing stdout: {e}");
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("Error reading from process stdout: {e}");
                    break;
                }
            }
        }
    });

    // Process stderr in a separate task
    debug!("spawning stderr handler");
    let stderr_handle = task::spawn(async move {
        let mut stderr_reader = BufReader::new(stderr);
        let mut buffer = [0u8; 1024];
        let mut stderr_writer = tokio::io::stderr();

        loop {
            match stderr_reader.read(&mut buffer).await {
                Ok(0) => break, // End of stderr
                Ok(n) => {
                    if let Err(e) = stderr_writer.write_all(&buffer[0..n]).await {
                        eprintln!("Error writing to stderr: {e}");
                        break;
                    }
                    if let Err(e) = stderr_writer.flush().await {
                        eprintln!("Error flushing stderr: {e}");
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("Error reading from process stderr: {e}");
                    break;
                }
            }
        }
    });

    // Spawn a task to wait for the child process to complete
    debug!("spawning exit code handler");
    let _keep_temp_file_clone = _keep_temp_file;
    tokio::spawn(async move {
        // Keep the temp file alive until the process completes
        let _temp_file_ref = _keep_temp_file_clone;

        // Wait for the child process to complete
        let status = match child.wait().await {
            Ok(status) => {
                debug!("Process exited with status: {:?}", status);
                status
            }
            Err(e) => {
                eprintln!("Error waiting for child process: {e}");
                // Send a default error code
                let _ = exit_code_tx.send(-1).await;
                return;
            }
        };

        // Wait for stdout/stderr tasks to complete
        if let Err(e) = stdout_handle.await {
            eprintln!("Error joining stdout task: {e}");
        }

        if let Err(e) = stderr_handle.await {
            eprintln!("Error joining stderr task: {e}");
        }

        // Send the exit code
        let exit_code = status.code().unwrap_or(-1);
        debug!("Sending exit code: {}", exit_code);
        let _ = exit_code_tx.send(exit_code).await;
    });

    // Return the communication channels as a ScriptSession
    Ok(ScriptSession {
        stdin_tx,
        exit_code_rx,
    })
}
