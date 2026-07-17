use super::StaticInitOptions;
use atuin_dotfiles::store::{AliasStore, var::VarStore};
use eyre::Result;

pub fn init_static(options: &StaticInitOptions<'_>) {
    let (bind_ctrl_r, bind_up_arrow) = if std::env::var("ATUIN_NOBIND").is_ok() {
        (false, false)
    } else {
        (options.enable_ctrl_r, options.enable_up_arrow)
    };

    // TODO: tmux popup for xonsh
    println!(
        "_ATUIN_BIND_CTRL_R={}",
        if bind_ctrl_r { "True" } else { "False" }
    );
    println!(
        "_ATUIN_BIND_UP_ARROW={}",
        if bind_up_arrow { "True" } else { "False" }
    );
    println!("{}", crate::shell::XONSH);
}

pub async fn init(
    aliases: AliasStore,
    vars: VarStore,
    options: &StaticInitOptions<'_>,
) -> Result<()> {
    init_static(options);

    let aliases = atuin_dotfiles::shell::xonsh::alias_config(&aliases).await;
    let vars = atuin_dotfiles::shell::xonsh::var_config(&vars).await;

    println!("{aliases}");
    println!("{vars}");

    Ok(())
}
