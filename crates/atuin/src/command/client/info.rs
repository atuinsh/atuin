use atuin_client::settings::Settings;

use crate::{SHA, VERSION};

pub fn run(settings: &Settings) {
    let config = atuin_common::utils::config_dir();
    let mut config_file = config.clone();
    config_file.push("config.toml");
    let mut sever_config = config;
    sever_config.push("server.toml");

    let config_paths = format!(
        "Config files:\nclient config: {:?}\nserver config: {:?}\nclient db path: {:?}\nkey path: {:?}\nsession path: {:?}",
        config_file.to_string_lossy(),
        sever_config.to_string_lossy(),
        settings.db_path,
        settings.key_path,
        settings.session_path
    );

    let env_vars = format!(
        "Env Vars:\nATUIN_CONFIG_DIR = {:?}",
        std::env::var("ATUIN_CONFIG_DIR").unwrap_or_else(|_| "None".into())
    );

    let general_info = format!("Version info:\nversion: {VERSION}\ncommit:  {SHA}");

    let print_out = format!("{config_paths}\n\n{env_vars}\n\n{general_info}");

    println!("{print_out}");
}
