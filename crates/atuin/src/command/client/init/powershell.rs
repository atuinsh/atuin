use super::StaticInitOptions;
use atuin_dotfiles::store::{AliasStore, var::VarStore};

pub fn init_static(options: &StaticInitOptions<'_>) {
    let (bind_ctrl_r, bind_up_arrow) = if std::env::var("ATUIN_NOBIND").is_ok() {
        (false, false)
    } else {
        (options.enable_ctrl_r, options.enable_up_arrow)
    };

    // TODO: tmux popup for Powershell
    println!("{}", crate::shell::POWERSHELL);
    println!(
        "Enable-AtuinSearchKeys -CtrlR {} -UpArrow {}",
        ps_bool(bind_ctrl_r),
        ps_bool(bind_up_arrow)
    );
}

pub async fn init(
    aliases: AliasStore,
    vars: VarStore,
    options: &StaticInitOptions<'_>,
) -> eyre::Result<()> {
    init_static(options);

    let aliases = atuin_dotfiles::shell::powershell::alias_config(&aliases).await;
    let vars = atuin_dotfiles::shell::powershell::var_config(&vars).await;

    println!("{aliases}");
    println!("{vars}");

    Ok(())
}

fn ps_bool(value: bool) -> &'static str {
    if value { "$true" } else { "$false" }
}
