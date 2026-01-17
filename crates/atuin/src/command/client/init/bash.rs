use atuin_client::settings::Tmux;
use atuin_dotfiles::store::{AliasStore, var::VarStore};
use eyre::Result;

fn print_tmux_config(tmux: &Tmux) {
    if tmux.enabled {
        println!("export ATUIN_TMUX_POPUP_WIDTH='{}'", tmux.width);
        println!("export ATUIN_TMUX_POPUP_HEIGHT='{}'", tmux.height);
    } else {
        println!("export ATUIN_TMUX_POPUP=false");
    }
}

pub fn init_static(disable_up_arrow: bool, disable_ctrl_r: bool, tmux: &Tmux) {
    let base = include_str!("../../../shell/atuin.bash");

    let (bind_ctrl_r, bind_up_arrow) = if std::env::var("ATUIN_NOBIND").is_ok() {
        (false, false)
    } else {
        (!disable_ctrl_r, !disable_up_arrow)
    };

    print_tmux_config(tmux);
    println!("__atuin_bind_ctrl_r={bind_ctrl_r}");
    println!("__atuin_bind_up_arrow={bind_up_arrow}");
    println!("{base}");
}

pub async fn init(
    aliases: AliasStore,
    vars: VarStore,
    disable_up_arrow: bool,
    disable_ctrl_r: bool,
    tmux: &Tmux,
) -> Result<()> {
    init_static(disable_up_arrow, disable_ctrl_r, tmux);

    let aliases = atuin_dotfiles::shell::bash::alias_config(&aliases).await;
    let vars = atuin_dotfiles::shell::bash::var_config(&vars).await;

    println!("{aliases}");
    println!("{vars}");

    Ok(())
}
