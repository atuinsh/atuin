use atuin_dotfiles::store::AliasStore;
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

pub async fn init(store: AliasStore, disable_up_arrow: bool, disable_ctrl_r: bool) -> Result<()> {
    init_static(disable_up_arrow, disable_ctrl_r);

    let aliases = atuin_dotfiles::shell::bash::config(&store).await;

    println!("{aliases}");

    Ok(())
}
