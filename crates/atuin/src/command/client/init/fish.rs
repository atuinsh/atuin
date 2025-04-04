use atuin_dotfiles::store::{AliasStore, var::VarStore};
use eyre::Result;

pub fn init_static(disable_up_arrow: bool, disable_ctrl_r: bool) {
    let base = include_str!("../../../shell/atuin.fish");

    println!("{base}");

    // In fish 4.0 and above the option bind -k doesn't exist anymore.
    // We keep it for compatibility with fish 3.x
    if std::env::var("ATUIN_NOBIND").is_err() {
        const BIND_CTRL_R: &str = r"bind \cr _atuin_search";
        const BIND_UP_ARROW: &str = r"bind -k up _atuin_bind_up
bind up _atuin_bind_up
bind \eOA _atuin_bind_up
bind \e\[A _atuin_bind_up";
        const BIND_CTRL_R_INS: &str = r"bind -M insert \cr _atuin_search";
        const BIND_UP_ARROW_INS: &str = r"bind -M insert -k up _atuin_bind_up
bind -M insert \eOA _atuin_bind_up
bind -M insert \e\[A _atuin_bind_up";

        if !disable_ctrl_r {
            println!("{BIND_CTRL_R}");
            println!("{BIND_CTRL_R_INS}");
        }
        if !disable_up_arrow {
            println!("{BIND_UP_ARROW}");
            println!("{BIND_UP_ARROW_INS}");
        }
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
