use atuin_client::settings::Settings;

use crate::{SHA, VERSION};

pub fn run(settings: &Settings) {
    let config_file = atuin_common::dirs::config_path("config.toml");
    let sever_config = atuin_common::dirs::config_path("server.toml");

    let config_paths = format!(
        "Config files:\nclient config: {:?}\nserver config: {:?}\nclient db path: {}\nkey path: \
         {}\nmeta db path: {}",
        config_file.to_string_lossy(),
        sever_config.to_string_lossy(),
        settings.db_path.display(),
        settings.key_path.display(),
        settings.meta.db_path.display()
    );

    let env_vars = std::fmt::from_fn(|f| {
        writeln!(f, "Env Vars:")?;
        for name in ["ATUIN_HOME", "ATUIN_CONFIG_DIR"] {
            writeln!(f, "{name} = {}", fmt_env(name))?;
        }
        Ok(())
    });

    let general_info = format!("Version info:\nversion: {VERSION}\ncommit:  {SHA}");

    let print_out = format!("{config_paths}\n\n{env_vars}\n{general_info}");

    println!("{print_out}");

    if let Some(legacy) = atuin_common::dirs::shadowed_legacy_config() {
        println!("\n{}", shadowed_config_warning(&legacy, &config_file));
    }
    if let Some(legacy) = atuin_common::dirs::shadowed_legacy_data_dir(&settings.data_dir) {
        println!("\n{}", shadowed_data_dir_warning(&legacy, &settings.data_dir));
    }
}

/// Atuin reads `active`, so the settings in `legacy` do nothing.
pub fn shadowed_config_warning(legacy: &std::path::Path, active: &std::path::Path) -> String {
    format!(
        "Warning: {} is ignored, because {} exists and Atuin reads that instead. Move your \
         settings into it, or delete it to go back to the old one.",
        legacy.display(),
        active.display()
    )
}

/// Atuin uses `active`, so the history and keys in `legacy` go unused.
pub fn shadowed_data_dir_warning(legacy: &std::path::Path, active: &std::path::Path) -> String {
    format!(
        "Warning: {} is ignored, because {} exists and Atuin uses that instead. If your history \
         is in the old one, move the new one somewhere else to go back to it.",
        legacy.display(),
        active.display()
    )
}

fn fmt_env(name: &str) -> impl std::fmt::Display {
    std::fmt::from_fn(move |f| {
        if let Some(value) = atuin_common::env::var_os(name) {
            #[expect(clippy::unnecessary_debug_formatting, reason = "escaping/quoting is desired")]
            (write!(f, "{value:?}"))
        } else {
            f.write_str("None")
        }
    })
}
