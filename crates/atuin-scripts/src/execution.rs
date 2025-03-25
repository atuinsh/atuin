use eyre::Result;
use crate::store::script::Script;
use std::process::{Stdio};
use tokio::io::{self, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task;
use std::fs;
use tempfile::NamedTempFile;

/// Represents the communication channels for an interactive script
pub struct ScriptSession {
    /// Channel to send input to the script
    stdin_tx: mpsc::Sender<String>,
    /// Exit code of the process once it completes
    exit_code_rx: mpsc::Receiver<i32>,
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

/// Execute a script interactively, allowing for ongoing stdin/stdout interaction
pub async fn execute_script_interactive(
    script: &Script,
) -> Result<ScriptSession, Box<dyn std::error::Error + Send + Sync>> {
    let interpreter = if !script.shebang.is_empty() {
        script.shebang.trim_start_matches("#!").trim().to_string()
    } else {
        "/bin/bash".to_string()
    };
    
    let parts: Vec<&str> = interpreter.split_whitespace().collect();
    let mut cmd = tokio::process::Command::new(parts[0]);
    
    for i in 1..parts.len() {
        cmd.arg(parts[i]);
    }

    // pretty annoying, but different interpreters have different ways to execute from string
    // handle those cases, fallback to just writing a temp file
    
    let interpreter_base = parts[0].split('/').last().unwrap_or(parts[0]);
    
    let mut temp_file_opt = None;
    
    match interpreter_base {
        "bash" | "sh" | "dash" | "zsh" | "ksh" => {
            cmd.arg("-c").arg(&script.script);
        },
        "python" | "python2" | "python3" => {
            cmd.arg("-c").arg(&script.script);
        },
        "perl" | "perl6" => {
            cmd.arg("-e").arg(&script.script);
        },
        "ruby" => {
            cmd.arg("-e").arg(&script.script);
        },
        "node" | "nodejs" => {
            cmd.arg("-e").arg(&script.script);
        },
        "php" => {
            cmd.arg("-r").arg(&script.script);
        },
        "pwsh" | "powershell" => {
            cmd.arg("-Command").arg(&script.script);
        },
        "R" | "Rscript" => {
            cmd.arg("-e").arg(&script.script);
        },
        _ => {
            
            let mut temp_file = NamedTempFile::new()?;
            let temp_path = temp_file.path().to_path_buf();
            
            // Write script content to the temp file
            tokio::fs::write(&temp_path, &script.script).await?;
            
            // Make it executable
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&temp_path)?.permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&temp_path, perms)?;
            }
            
            // Use the temp file as the script
            cmd.arg(temp_path.to_str().unwrap());
            
            // Store temp file to prevent it from being dropped early
            temp_file_opt = Some(temp_file);
        }
    }
    
    // Configure the command for interactive mode
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    
    // Get handles to stdin, stdout, stderr
    let mut stdin = child.stdin.take()
        .ok_or_else(|| "Failed to open child process stdin".to_string())?;
    let stdout = child.stdout.take()
        .ok_or_else(|| "Failed to open child process stdout".to_string())?;
    let stderr = child.stderr.take()
        .ok_or_else(|| "Failed to open child process stderr".to_string())?;
    
    // Create channels for the interactive session
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(32);
    let (exit_code_tx, exit_code_rx) = mpsc::channel::<i32>(1);
    
    // handle user stdin
    tokio::spawn(async move {
        while let Some(input) = stdin_rx.recv().await {
            if let Err(e) = stdin.write_all(input.as_bytes()) {
                eprintln!("Error writing to stdin: {}", e);
                break;
            }
            if let Err(e) = stdin.flush(){
                eprintln!("Error flushing stdin: {}", e);
                break;
            }
        }
        // when the channel closes (sender dropped), we let stdin close naturally
    });
    
    // handle stdout
    let stdout_handle = task::spawn(async move {
        let mut stdout_reader = BufReader::new(stdout);
        let mut buffer = [0u8; 1024];
        let mut stdout_writer = tokio::io::stdout();
        
        loop {
            match stdout_reader.read(&mut buffer).await {
                Ok(0) => break, // End of stdout
                Ok(n) => {
                    if let Err(e) = stdout_writer.write_all(&buffer[0..n]).await {
                        eprintln!("Error writing to stdout: {}", e);
                        break;
                    }
                    if let Err(e) = stdout_writer.flush().await {
                        eprintln!("Error flushing stdout: {}", e);
                        break;
                    }
                },
                Err(e) => {
                    eprintln!("Error reading from process stdout: {}", e);
                    break;
                }
            }
        }
    });
    
    // Process stderr in a separate task
    let stderr_handle = task::spawn(async move {
        let mut stderr_reader = BufReader::new(stderr);
        let mut buffer = [0u8; 1024];
        let mut stderr_writer = tokio::io::stderr();
        
        loop {
            match stderr_reader.read(&mut buffer).await {
                Ok(0) => break, // End of stderr
                Ok(n) => {
                    if let Err(e) = stderr_writer.write_all(&buffer[0..n]).await {
                        eprintln!("Error writing to stderr: {}", e);
                        break;
                    }
                    if let Err(e) = stderr_writer.flush().await {
                        eprintln!("Error flushing stderr: {}", e);
                        break;
                    }
                },
                Err(e) => {
                    eprintln!("Error reading from process stderr: {}", e);
                    break;
                }
            }
        }
    });
    
    // Spawn a task to wait for the child process to complete
    tokio::spawn(async move {
        // Keep the temp file alive until the process completes
        let _keep_temp_file = temp_file_opt;
        
        // Wait for the child process to complete
        let status = match child.wait().await {
            Ok(status) => status,
            Err(e) => {
                eprintln!("Error waiting for child process: {}", e);
                // Send a default error code
                let _ = exit_code_tx.send(-1).await;
                return;
            }
        };
        
        // Wait for stdout/stderr tasks to complete
        if let Err(e) = stdout_handle.await {
            eprintln!("Error joining stdout task: {}", e);
        }
        
        if let Err(e) = stderr_handle.await {
            eprintln!("Error joining stderr task: {}", e);
        }
        
        // Send the exit code
        let _ = exit_code_tx.send(status.code().unwrap_or(-1)).await;
    });
    
    // Return the communication channels as a ScriptSession
    Ok(ScriptSession {
        stdin_tx,
        exit_code_rx,
    })
}
