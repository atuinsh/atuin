use super::Emit;
use atuin_dotfiles::store::{AliasStore, var::VarStore};
use eyre::Result;

pub fn init_static(disable_up_arrow: bool, disable_ctrl_r: bool) {
    let base = include_str!("../../../shell/atuin.xsh");

    let (bind_ctrl_r, bind_up_arrow) = if std::env::var("ATUIN_NOBIND").is_ok() {
        (false, false)
    } else {
        (!disable_ctrl_r, !disable_up_arrow)
    };
    println!(
        "_ATUIN_BIND_CTRL_R={}",
        if bind_ctrl_r { "True" } else { "False" }
    );
    println!(
        "_ATUIN_BIND_UP_ARROW={}",
        if bind_up_arrow { "True" } else { "False" }
    );
    println!("{base}");
}

pub async fn init(
    aliases: AliasStore,
    vars: VarStore,
    disable_up_arrow: bool,
    disable_ctrl_r: bool,
    emit: Emit,
) -> Result<()> {
    match emit {
        Emit::All => {
            init_static(disable_up_arrow, disable_ctrl_r);
            let vars = atuin_dotfiles::shell::xonsh::var_config(&vars).await;
            println!("{vars}");
            let aliases = atuin_dotfiles::shell::xonsh::alias_config(&aliases).await;
            println!("{aliases}");
        }
        Emit::Necessary => {
            init_static(disable_up_arrow, disable_ctrl_r);
        }
        Emit::EnvOnly => {
            let vars = atuin_dotfiles::shell::xonsh::var_config(&vars).await;
            println!("{vars}");
        }
        Emit::AliasesOnly => {
            let aliases = atuin_dotfiles::shell::xonsh::alias_config(&aliases).await;
            println!("{aliases}");
        }
    }

    Ok(())
}
