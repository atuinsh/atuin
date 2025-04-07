use atuin_dotfiles::store::{AliasStore, var::VarStore};
use eyre::Result;

pub fn init_static(disable_up_arrow: bool, disable_ctrl_r: bool) {
    let base = include_str!("../../../shell/atuin.fish");

    println!("{base}");

    // In fish 4.0 and above the option bind -k doesn't exist anymore.
    // We keep it for compatibility with fish 3.x
    if std::env::var("ATUIN_NOBIND").is_err() {
        const BIND_CTRL_R: &str = r"bind \cr _atuin_search";
        const BIND_CTRL_R_INS: &str = r"bind -M insert \cr _atuin_search";
        const BIND_UP_ARROW_INS: &str = r"bind -M insert -k up _atuin_bind_up
bind -M insert \eOA _atuin_bind_up
bind -M insert \e\[A _atuin_bind_up";

        let bind_up_arrow = match std::env::var("FISH_VERSION") {
            Ok(ref version) if version.starts_with("4.") => r"bind up _atuin_bind_up",
            Ok(_) => r"bind -k up _atuin_bind_up",

            // do nothing - we can't panic or error as this could be in use in
            // non-fish pipelines
            _ => "",
        }
        .to_string();

        if !disable_ctrl_r {
            println!("{BIND_CTRL_R}");
        }
        if !disable_up_arrow {
            println!(
                r"{bind_up_arrow}
bind \eOA _atuin_bind_up
bind \e\[A _atuin_bind_up"
            );
        }

        println!("if bind -M insert > /dev/null 2>&1");
        if !disable_ctrl_r {
            println!("{BIND_CTRL_R_INS}");
        }
        if !disable_up_arrow {
            println!("{BIND_UP_ARROW_INS}");
        }
        println!("end");
    }
}

pub async fn init(
    aliases: AliasStore,
    vars: VarStore,
    disable_up_arrow: bool,
    disable_ctrl_r: bool,
) -> Result<()> {
    init_static(disable_up_arrow, disable_ctrl_r);

    let aliases = atuin_dotfiles::shell::fish::alias_config(&aliases).await;
    let vars = atuin_dotfiles::shell::fish::var_config(&vars).await;

    println!("{aliases}");
    println!("{vars}");

    Ok(())
}
