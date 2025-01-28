use atuin_dotfiles::store::{var::VarStore, AliasStore};
use eyre::Result;

pub fn init_static(disable_up_arrow: bool, disable_ctrl_r: bool) {
    let base = include_str!("../../../shell/atuin.bash");

    let (bind_ctrl_r, bind_up_arrow) = if std::env::var("ATUIN_NOBIND").is_ok() {
        (false, false)
    } else {
        (!disable_ctrl_r, !disable_up_arrow)
    };

    println!("__atuin_bind_ctrl_r={bind_ctrl_r}");
    println!("__atuin_bind_up_arrow={bind_up_arrow}");
    println!("{base}");
}

pub async fn init(
    aliases: AliasStore,
    vars: VarStore,
    disable_up_arrow: bool,
    disable_ctrl_r: bool,
    only_env: bool,
    skip_env: bool,
) -> Result<()> {
    if !only_env {
        init_static(disable_up_arrow, disable_ctrl_r);

        let aliases = atuin_dotfiles::shell::bash::alias_config(&aliases).await;
        println!("{aliases}");
    }

    if !skip_env {
        let vars = atuin_dotfiles::shell::bash::var_config(&vars).await;
        println!("{vars}");
    }

    Ok(())
}
