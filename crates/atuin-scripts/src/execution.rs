use std::collections::{HashMap, HashSet};
use std::process::Stdio;

use eyre::Result;
use tempfile::NamedTempFile;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task;
use tracing::debug;

use crate::store::script::Script;

// Helper function to build a complete script with shebang
#[must_use]
pub fn build_executable_script(script: &str, shebang: &str) -> String {
    if shebang.is_empty() {
        // Default to bash if no shebang is provided
        format!("#!/usr/bin/env bash\n{script}")
    } else if shebang.starts_with("#!") {
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

fn is_env_split_string_option(option: &str) -> bool {
    // Only flags without arguments can precede S in a short-option cluster.
    // An S in the value of -u, -C, or -a is not a split-string option.
    option.starts_with("--split-string=")
        || option
            .strip_prefix('-')
            .is_some_and(|flags| flags.trim_start_matches(['i', 'v', '0']).starts_with('S'))
}

/// Execute a script interactively, allowing for ongoing stdin/stdout interaction
pub async fn execute_script_interactive(
    script: String,
    shebang: String,
) -> Result<ScriptSession, Box<dyn std::error::Error + Send + Sync>> {
    // Create a temporary file for the script
    // Close the writable handle before exec to avoid ETXTBSY on Linux, while
    // keeping the path alive until the child exits.
    let temp_file = NamedTempFile::new()?.into_temp_path();
    let temp_path = temp_file.to_path_buf();

    debug!("creating temp file at {}", temp_path.display());

    // Extract interpreter from shebang for fallback execution
    let interpreter = if shebang.is_empty() {
        "/usr/bin/env bash".to_string()
    } else {
        shebang.trim_start_matches("#!").lines().next().unwrap_or_default().trim().to_string()
    };
    let parts: Vec<&str> = interpreter.split_whitespace().collect();

    // Write script content to the temp file, including the shebang
    let full_script_content = build_executable_script(&script, &shebang);

    debug!("writing script content to temp file");
    tokio::fs::write(&temp_path, &full_script_content).await?;

    // Make it executable on Unix systems
    #[cfg(unix)]
    {
        debug!("making script executable");
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&temp_path)?.permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&temp_path, perms)?;
    }

    // Store the temp_file to prevent it from being dropped
    // This ensures it won't be deleted while the script is running
    let keep_temp_file = temp_file;

    // Linux passes everything after the interpreter as one argument. Invoke multi-argument
    // interpreters explicitly, keeping the complete script on disk. env -S handles its own
    // splitting, including quoting, so those shebangs should still go through the kernel.
    let env_splits_arguments =
        parts.first().is_some_and(|p| std::path::Path::new(p).file_name() == Some("env".as_ref()))
            && parts.get(1).is_some_and(|arg| is_env_split_string_option(arg));
    let interpreter_command = || {
        let mut command = tokio::process::Command::new(parts[0]);
        if env_splits_arguments {
            // Match the kernel's single shebang argument so env can parse its own quoting.
            command.arg(interpreter[parts[0].len()..].trim());
        } else {
            command.args(&parts[1..]);
        }
        command.arg(&temp_path);
        command
    };
    let explicit_interpreter = parts.len() > 2 && !env_splits_arguments;
    let mut command = if explicit_interpreter {
        debug!("executing interpreter with separate arguments");
        interpreter_command()
    } else {
        debug!("attempting direct script execution");
        tokio::process::Command::new(&temp_path)
    };
    let mut child_result =
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();

    // If direct execution fails, try using the interpreter
    if let Err(e) = &child_result
        && !explicit_interpreter
        && !parts.is_empty()
    {
        debug!("direct execution failed: {}, trying with interpreter", e);
        child_result = interpreter_command()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
    }

    // If it still fails, return the error
    let mut child = match child_result {
        Ok(child) => child,
        Err(e) => {
            return Err(format!("Failed to execute script: {e}").into());
        }
    };

    // Get handles to stdin, stdout, stderr
    let mut stdin =
        child.stdin.take().ok_or_else(|| "Failed to open child process stdin".to_string())?;
    let stdout =
        child.stdout.take().ok_or_else(|| "Failed to open child process stdout".to_string())?;
    let stderr =
        child.stderr.take().ok_or_else(|| "Failed to open child process stderr".to_string())?;

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
    tokio::spawn(async move {
        // Keep the temp file alive until the process completes
        let _temp_file_ref = keep_temp_file;

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

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::build_executable_script;

    #[rstest]
    #[case::short("-S", true)]
    #[case::short_attached("-Ssh", true)]
    #[case::long_attached("--split-string=sh", true)]
    #[case::ignore_environment("-iS", true)]
    #[case::debug("-ivS", true)]
    #[case::reordered_flags("-viSsh", true)]
    #[case::null("-0S", true)]
    #[case::unset_value("-uSESSION", false)]
    #[case::chdir_value("-CScripts", false)]
    #[case::argv0_value("-aSHELL", false)]
    #[case::long_unset_value("--unset=SESSION", false)]
    #[case::missing_value("--split-string", false)]
    fn env_split_flags(#[case] option: &str, #[case] expected: bool) {
        assert_eq!(super::is_env_split_string_option(option), expected);
    }

    #[cfg(unix)]
    async fn script_exit(script: String, shebang: &str) -> Option<i32> {
        let mut session = super::execute_script_interactive(script, shebang.to_owned())
            .await
            .expect("script should start");
        tokio::time::timeout(std::time::Duration::from_secs(5), session.wait_for_exit())
            .await
            .expect("script should exit")
    }

    #[cfg(unix)]
    #[rstest]
    #[case::path_lookup("sh")]
    #[case::path_lookup_with_flag("sh -e")]
    #[tokio::test]
    async fn execution_falls_back_to_an_interpreter_on_path(#[case] shebang: &str) {
        assert_eq!(script_exit("exit 7\n".to_owned(), shebang).await, Some(7));
    }

    #[cfg(unix)]
    #[rstest]
    #[case::default_bash("", false, false)]
    #[case::bare_interpreter("/bin/sh", false, false)]
    #[case::prefixed_interpreter("#!/usr/bin/env sh", false, false)]
    // The kernel cannot find a relative interpreter; PATH lookup forces the fallback.
    #[case::fallback_preserves_header("sh", false, false)]
    #[case::env_bash("#!/usr/bin/env bash -e", false, true)]
    #[case::bare_env_bash("/usr/bin/env bash -e", false, true)]
    #[case::env_multiple_args("#!/usr/bin/env sh -e -u", true, true)]
    #[case::direct_multiple_args("#!/bin/sh -e -u", true, true)]
    #[case::multiline_header(
        "#!/usr/bin/env sh -e\n#! nix shell nixpkgs#bash --command bash",
        false,
        true
    )]
    #[tokio::test]
    async fn execution_preserves_interpreter_arguments(
        #[case] shebang: &str,
        #[case] nounset: bool,
        #[case] errexit: bool,
    ) {
        let header = build_executable_script("", shebang);
        let header = header.lines().next().unwrap();
        let pragma = "#! nix shell nixpkgs#bash --command bash";
        let script = format!(
            "{pragma}\n{{ IFS= read -r first; IFS= read -r second; }} < \"$0\"\n[ \"$first\" = \
             '{header}' ] && [ \"$second\" = '{pragma}' ] || exit 9\nif {nounset}; then case $- \
             in *u*) ;; *) exit 8 ;; esac; fi\nfalse\nexit 0\n"
        );
        assert_eq!(
            script_exit(script, shebang).await,
            Some(i32::from(errexit)),
            "all interpreter flags and script headers must be preserved"
        );
    }

    #[cfg(target_os = "linux")]
    #[rstest]
    #[case::short("-S ")]
    #[case::long("--split-string=")]
    #[case::combined_ignore_environment("-iS ")]
    #[case::combined_debug("-ivS ")]
    #[tokio::test]
    async fn native_env_split_preserves_quoted_arguments(#[case] split_flag: &str) {
        let shebang =
            format!(r#"#!/usr/bin/env {split_flag}sh -c 'test "$0" = "two words"' 'two words'"#);
        assert_eq!(script_exit("exit 9\n".to_owned(), &shebang).await, Some(0));
    }

    #[rstest]
    #[case::default_bash("", "#!/usr/bin/env bash")]
    #[case::bare_interpreter("/usr/bin/env python3", "#!/usr/bin/env python3")]
    #[case::prefixed_interpreter("#!/usr/bin/env python3", "#!/usr/bin/env python3")]
    fn builds_executable_script(
        #[case] shebang: &str,
        #[case] expected_shebang: &str,
        #[values("print(1 + 1)\n", "#!/usr/bin/env python3\nprint(1 + 1)\n")] script: &str,
    ) {
        assert_eq!(
            build_executable_script(script, shebang),
            format!("{expected_shebang}\n{script}")
        );
    }
}
