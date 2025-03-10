use atuin_dotfiles::store::{var::VarStore, AliasStore};

pub fn init_static(disable_up_arrow: bool, disable_ctrl_r: bool) {
    let base = include_str!("../../../shell/atuin.ps1");

    let (bind_ctrl_r, bind_up_arrow) = if std::env::var("ATUIN_NOBIND").is_ok() {
        (false, false)
    } else {
        (!disable_ctrl_r, !disable_up_arrow)
    };

    println!("{base}");
    println!(
        "Enable-AtuinSearchKeys -CtrlR {} -UpArrow {}",
        ps_bool(bind_ctrl_r),
        ps_bool(bind_up_arrow)
    );
}

pub async fn init(
    aliases: AliasStore,
    vars: VarStore,
    disable_up_arrow: bool,
    disable_ctrl_r: bool,
) -> eyre::Result<()> {
    init_static(disable_up_arrow, disable_ctrl_r);

    let aliases = atuin_dotfiles::shell::powershell::alias_config(&aliases).await;
    let vars = atuin_dotfiles::shell::powershell::var_config(&vars).await;

    println!("{aliases}");
    println!("{vars}");

    Ok(())
}

fn ps_bool(value: bool) -> &'static str {
    if value {
        "$true"
    } else {
        "$false"
    }
}
