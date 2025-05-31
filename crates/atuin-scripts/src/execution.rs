use crate::store::script::Script;
use eyre::Result;
use std::collections::{HashMap, HashSet};
use std::fs;
use tempfile::NamedTempFile;
use tokio::sync::mpsc;
use tracing::debug;

// Helper function to build a complete script with shebang
pub fn build_executable_script(script: String, shebang: String) -> String {
    if shebang.is_empty() {
        // Default to bash if no shebang is provided
        format!("#!/usr/bin/env bash\n{}", script)
    } else if script.starts_with("#!") {
        format!("{}\n{}", shebang, script)
    } else {
        format!("#!{}\n{}", shebang, script)
    }
}

/// Represents the communication channels for an interactive script
pub struct ScriptSession {
    /// Indicate that parent is being killed
    pub killer_tx: mpsc::Sender<bool>,
    /// Exit code of the process once it completes
    pub exit_code_rx: mpsc::Receiver<i32>,
}

impl ScriptSession {
    // return the sender for the canceler
    pub async fn get_canceler(&mut self) -> mpsc::Sender<bool> {
        self.killer_tx.clone()
    }
    /// Wait for the script to complete and get the exit code
    pub async fn wait_for_exit(&mut self) -> Option<i32> {
        self.exit_code_rx.recv().await
    }
}

fn setup_template(script: &Script) -> Result<minijinja::Environment> {
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
    let mut env = setup_template(script)?;

    // Add the default function - it has no value here, all variables will be provided in context
    env.add_function("default", move |name: String, value: String| -> String {
        format!("{name}={value}")
    });

    let template = env.get_template("script")?;
    let rendered = template.render(context)?;

    Ok(rendered)
}

/// Get the variables that need to be templated in a script
pub fn template_variables(script: &Script) -> Result<(HashSet<String>, HashMap<String, String>)> {
    use std::sync::{Arc, Mutex};

    let mut env = setup_template(script)?;

    // Use Arc<Mutex> for thread-safe shared mutable state
    let defaults = Arc::new(Mutex::new(HashMap::new()));
    let defaults_clone = defaults.clone();

    // Add the default function to collect default values
    env.add_function("default", move |name: String, value: String| -> String {
        if let Ok(mut defaults_guard) = defaults_clone.lock() {
            defaults_guard.insert(name.clone(), value.clone());
        }
        println!("default called with {}={}", name, value);
        format!("{name}={value}")
    });

    let template = env.get_template("script")?;
    // Get all undeclared variables
    let mut undeclared = template.undeclared_variables(true);
    // Remove the default function from undeclared variables
    undeclared.remove("default");

    if undeclared.is_empty() {
        return Ok((HashSet::new(), HashMap::new()));
    }

    // Render the template with empty context to trigger default function calls
    let _rendered = template.render(minijinja::context! {})?;

    let defaults_map = defaults
        .lock()
        .map_err(|_| eyre::eyre!("Failed to acquire mutex"))?
        .clone();

    Ok((undeclared, defaults_map))
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
        let mut perms = fs::metadata(&temp_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&temp_path, perms)?;
    }

    // Store the temp_file to prevent it from being dropped
    // This ensures it won't be deleted while the script is running
    let _keep_temp_file = temp_file;

    debug!("attempting direct script execution");
    let mut child_result = tokio::process::Command::new(temp_path.to_str().unwrap()).spawn();

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
            child_result = cmd.spawn();
        }
    }

    // If it still fails, return the error
    let mut child = match child_result {
        Ok(child) => child,
        Err(e) => {
            return Err(format!("Failed to execute script: {}", e).into());
        }
    };

    // Create channels for the interactive session
    let (killer_tx, mut killer_rx) = mpsc::channel::<bool>(1);
    let (exit_code_tx, exit_code_rx) = mpsc::channel::<i32>(1);

    // Spawn a task to wait for the child process to complete
    debug!("spawning exit code handler");
    let _keep_temp_file_clone = _keep_temp_file;
    tokio::spawn(async move {
        use tokio::select;

        // Keep the temp file alive until the process completes
        let _temp_file_ref = _keep_temp_file_clone;

        // wait for child process to end or parent to receive Ctrl-C
        let status = select! {
            // check if the child process has exited
            status = child.wait() => {
                debug!("Child process exited");
                match status {
                    Ok(status) => {
                        debug!("Child process exited with status: {:?}", status);
                        status
                    }
                    Err(e) => {
                        eprintln!("Error waiting for child process: {}", e);
                        let _ = exit_code_tx.send(-1).await;
                        return;
                    }
                }
            }
            // Check if parent it being terminated
            _ = killer_rx.recv() => {
                debug!("Received killer signal, terminating child process");
                match child.kill().await {
                    Ok(_) => {
                        debug!("Child process was killed");
                    }
                    Err(e) => {
                        eprintln!("Error killing child process: {}", e);
                    }
                }
                let _ = exit_code_tx.send(-1).await;
                return;
            }
        };

        // Send the exit code
        let exit_code = status.code().unwrap_or(-1);
        debug!("Sending exit code: {}", exit_code);
        let _ = exit_code_tx.send(exit_code).await;
    });

    // Return the communication channels as a ScriptSession
    Ok(ScriptSession {
        killer_tx,
        exit_code_rx,
    })
}
