use std::path::PathBuf;

use atuin_scripts::{
    execution::{build_executable_script, execute_script_interactive},
    store::{ScriptStore, script::Script},
};
use clap::{Parser, Subcommand};
use eyre::{Result, bail};
use tempfile::NamedTempFile;

use atuin_client::{database::Database, record::sqlite_store::SqliteStore, settings::Settings};
use tracing::debug;

#[derive(Parser, Debug)]
pub struct NewScript {
    #[arg(short, long)]
    pub name: String,

    #[arg(short, long)]
    pub description: Option<String>,

    #[arg(short, long)]
    pub tags: Vec<String>,

    #[arg(short, long)]
    pub shebang: Option<String>,

    #[arg(long)]
    pub script: Option<PathBuf>,

    #[allow(clippy::option_option)]
    #[arg(long)]
    /// Use the last command as the script content
    /// Optionally specify a number to use the last N commands
    pub last: Option<Option<usize>>,
    
    #[arg(long)]
    /// Skip opening editor when using --last
    pub no_edit: bool,
}

#[derive(Parser, Debug)]
pub struct Run {
    pub name: String,
}

#[derive(Parser, Debug)]
pub struct List {}

#[derive(Parser, Debug)]
pub struct Get {
    pub name: String,

    #[arg(short, long)]
    /// Display only the executable script with shebang
    pub script: bool,
}

#[derive(Parser, Debug)]
pub struct Edit {
    pub name: String,

    #[arg(short, long)]
    pub description: Option<String>,

    /// Replace all existing tags with these new tags
    #[arg(short, long)]
    pub tags: Vec<String>,
    
    /// Remove all tags from the script
    #[arg(long)]
    pub no_tags: bool,
    
    /// Rename the script
    #[arg(long)]
    pub rename: Option<String>,

    #[arg(short, long)]
    pub shebang: Option<String>,

    #[arg(long)]
    pub script: Option<PathBuf>,
    
    /// Skip opening editor
    #[arg(long)]
    pub no_edit: bool,
}

#[derive(Parser, Debug)]
pub struct Delete {
    pub name: String,

    #[arg(short, long)]
    pub force: bool,
}

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    New(NewScript),
    Run(Run),
    List(List),

    Get(Get),
    Edit(Edit),
    Delete(Delete),
}

impl Cmd {
    // Helper function to open an editor with optional initial content
    fn open_editor(initial_content: Option<&str>) -> Result<String> {
        // Create a temporary file
        let temp_file = NamedTempFile::new()?;
        let path = temp_file.into_temp_path();
        
        // Write initial content to the temp file if provided
        if let Some(content) = initial_content {
            std::fs::write(&path, content)?;
        }
        
        // Open the file in the user's preferred editor
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
        let status = std::process::Command::new(editor).arg(&path).status()?;
        if !status.success() {
            bail!("failed to open editor");
        }
        
        // Read back the edited content
        let content = std::fs::read_to_string(&path)?;
        path.close()?;
        
        Ok(content)
    }

    async fn handle_new_script(
        settings: &Settings,
        new_script: NewScript,
        script_store: ScriptStore,
        script_db: atuin_scripts::database::Database,
        history_db: &impl Database,
    ) -> Result<()> {
        let script_content = if let Some(count_opt) = new_script.last {
            // Get the last N commands from history, plus 1 to exclude the command that runs this script
            let count = count_opt.unwrap_or(1) + 1; // Add 1 to the count to exclude the current command
            let context = atuin_client::database::current_context();

            // Get the last N+1 commands, filtering by the default mode
            let filters = [settings.default_filter_mode()];

            let mut history = history_db
                .list(&filters, &context, Some(count), false, false)
                .await?;

            // Reverse to get chronological order
            history.reverse();

            // Skip the most recent command (which would be the atuin scripts new command itself)
            if !history.is_empty() {
                history.pop(); // Remove the most recent command
            }

            // Format the commands into a script
            let commands: Vec<String> = history.iter().map(|h| h.command.clone()).collect();

            if commands.is_empty() {
                bail!("No commands found in history");
            }

            let script_text = commands.join("\n");
            
            // Only open editor if --no-edit is not specified
            if new_script.no_edit {
                Some(script_text)
            } else {
                // Open the editor with the commands pre-loaded
                Some(Self::open_editor(Some(&script_text))?)
            }
        } else if let Some(script_path) = new_script.script {
            let script_content = std::fs::read_to_string(script_path)?;
            Some(script_content)
        } else {
            // Open editor with empty file
            Some(Self::open_editor(None)?)
        };

        let script = Script::builder()
            .name(new_script.name)
            .description(new_script.description.unwrap_or_default())
            .shebang(new_script.shebang.unwrap_or_default())
            .tags(new_script.tags)
            .script(script_content.unwrap_or_default())
            .build();

        script_store.create(script).await?;

        script_store.build(script_db).await?;

        Ok(())
    }

    async fn handle_run(
        _settings: &Settings,
        run: Run,
        script_db: atuin_scripts::database::Database,
    ) -> Result<()> {
        let script = script_db.get_by_name(&run.name).await?;

        if let Some(script) = script {
            let mut session = execute_script_interactive(&script)
                .await
                .expect("failed to execute script");

            // Create a channel to signal when the process exits
            let (exit_tx, mut exit_rx) = tokio::sync::oneshot::channel();

            // Set up a task to read from stdin and forward to the script
            let sender = session.stdin_tx.clone();
            let stdin_task = tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                use tokio::select;

                let stdin = tokio::io::stdin();
                let mut reader = tokio::io::BufReader::new(stdin);
                let mut buffer = vec![0u8; 1024]; // Read in chunks for efficiency

                loop {
                    // Use select to either read from stdin or detect when the process exits
                    select! {
                        // Check if the script process has exited
                        _ = &mut exit_rx => {
                            break;
                        }
                        // Try to read from stdin
                        read_result = reader.read(&mut buffer) => {
                            match read_result {
                                Ok(0) => break, // EOF
                                Ok(n) => {
                                    // Convert the bytes to a string and forward to script
                                    let input = String::from_utf8_lossy(&buffer[0..n]).to_string();
                                    if let Err(e) = sender.send(input).await {
                                        eprintln!("Error sending input to script: {e}");
                                        break;
                                    }
                                },
                                Err(e) => {
                                    eprintln!("Error reading from stdin: {e}");
                                    break;
                                }
                            }
                        }
                    }
                }
            });

            // Wait for the script to complete
            let exit_code = session.wait_for_exit().await;

            // Signal the stdin task to stop
            let _ = exit_tx.send(());
            let _ = stdin_task.await;

            if let Some(code) = exit_code {
                if code != 0 {
                    eprintln!("Script exited with code {code}");
                }
            }
        } else {
            bail!("script not found");
        }
        Ok(())
    }

    async fn handle_list(
        _settings: &Settings,
        _list: List,
        script_db: atuin_scripts::database::Database,
    ) -> Result<()> {
        let scripts = script_db.list().await?;

        if scripts.is_empty() {
            println!("No scripts found");
        } else {
            println!("Available scripts:");
            for script in scripts {
                if script.tags.is_empty() {
                    println!("- {} ", script.name);
                } else {
                    println!("- {} [tags: {}]", script.name, script.tags.join(", "));
                }

                // Print description if it's not empty
                if !script.description.is_empty() {
                    println!("  Description: {}", script.description);
                }
            }
        }

        Ok(())
    }

    async fn handle_get(
        _settings: &Settings,
        get: Get,
        script_db: atuin_scripts::database::Database,
    ) -> Result<()> {
        let script = script_db.get_by_name(&get.name).await?;

        if let Some(script) = script {
            if get.script {
                // Just print the executable script with shebang
                print!("{}", build_executable_script(&script));
                return Ok(());
            }

            // Create a YAML representation of the script
            println!("---");
            println!("name: {}", script.name);
            println!("id: {}", script.id);

            if script.description.is_empty() {
                println!("description: \"\"");
            } else {
                println!("description: |");
                // Indent multiline descriptions properly for YAML
                for line in script.description.lines() {
                    println!("  {line}");
                }
            }

            if script.tags.is_empty() {
                println!("tags: []");
            } else {
                println!("tags:");
                for tag in &script.tags {
                    println!("  - {tag}");
                }
            }

            println!("shebang: {}", script.shebang);

            println!("script: |");
            // Indent the script content for proper YAML multiline format
            for line in script.script.lines() {
                println!("  {line}");
            }

            Ok(())
        } else {
            bail!("script '{}' not found", get.name);
        }
    }

    async fn handle_edit(
        _settings: &Settings,
        edit: Edit,
        script_store: ScriptStore,
        script_db: atuin_scripts::database::Database,
    ) -> Result<()> {
        debug!("editing script {:?}", edit);
        // Find the existing script
        let existing_script = script_db.get_by_name(&edit.name).await?;
        debug!("existing script {:?}", existing_script);

        if let Some(mut script) = existing_script {
            // Update the script with new values if provided
            if let Some(description) = edit.description {
                script.description = description;
            }
            
            // Handle renaming if requested
            if let Some(new_name) = edit.rename {
                // Check if a script with the new name already exists
                if let Some(_) = script_db.get_by_name(&new_name).await? {
                    bail!("A script named '{}' already exists", new_name);
                }
                
                // Update the name
                script.name = new_name;
            }

            // Handle tag updates with priority:
            // 1. If --no-tags is provided, clear all tags
            // 2. If --tags is provided, replace all tags
            // 3. If neither is provided, tags remain unchanged
            if edit.no_tags {
                // Clear all tags
                script.tags.clear();
            } else if !edit.tags.is_empty() {
                // Replace all tags
                script.tags = edit.tags;
            }
            // If none of the above conditions are met, tags remain unchanged

            if let Some(shebang) = edit.shebang {
                script.shebang = shebang;
            }

            // Handle script content update
            let script_content = if let Some(script_path) = edit.script {
                // Load script from provided file
                std::fs::read_to_string(script_path)?
            } else if !edit.no_edit {
                // Open the script in editor for interactive editing if --no-edit is not specified
                Self::open_editor(Some(&script.script))?
            } else {
                // If --no-edit is specified, keep the existing script content
                script.script.clone()
            };

            // Update the script content
            script.script = script_content;

            // Update the script in the store
            script_store.update(script).await?;

            // Rebuild the database to apply changes
            script_store.build(script_db).await?;

            println!("Script '{}' updated successfully!", edit.name);

            Ok(())
        } else {
            bail!("script '{}' not found", edit.name);
        }
    }

    async fn handle_delete(
        _settings: &Settings,
        delete: Delete,
        script_store: ScriptStore,
        script_db: atuin_scripts::database::Database,
    ) -> Result<()> {
        // Find the script by name
        let script = script_db.get_by_name(&delete.name).await?;

        if let Some(script) = script {
            // If not force, confirm deletion
            if !delete.force {
                println!(
                    "Are you sure you want to delete script '{}'? [y/N]",
                    delete.name
                );
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;

                let input = input.trim().to_lowercase();
                if input != "y" && input != "yes" {
                    println!("Deletion cancelled");
                    return Ok(());
                }
            }

            // Delete the script
            script_store.delete(script.id).await?;

            // Rebuild the database to apply changes
            script_store.build(script_db).await?;

            println!("Script '{}' deleted successfully", delete.name);
            Ok(())
        } else {
            bail!("script '{}' not found", delete.name);
        }
    }

    pub async fn run(
        self,
        settings: &Settings,
        store: SqliteStore,
        history_db: &impl Database,
    ) -> Result<()> {
        let host_id = Settings::host_id().expect("failed to get host_id");
        let encryption_key: [u8; 32] = atuin_client::encryption::load_key(settings)?.into();

        let script_store = ScriptStore::new(store, host_id, encryption_key);
        let script_db =
            atuin_scripts::database::Database::new(settings.scripts.database_path.clone(), 1.0)
                .await?;

        match self {
            Self::New(new_script) => {
                Self::handle_new_script(settings, new_script, script_store, script_db, history_db)
                    .await
            }
            Self::Run(run) => Self::handle_run(settings, run, script_db).await,
            Self::List(list) => Self::handle_list(settings, list, script_db).await,
            Self::Get(get) => Self::handle_get(settings, get, script_db).await,
            Self::Edit(edit) => Self::handle_edit(settings, edit, script_store, script_db).await,
            Self::Delete(delete) => {
                Self::handle_delete(settings, delete, script_store, script_db).await
            }
        }
    }
}
