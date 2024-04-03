use atuin_dotfiles::store::AliasStore;
use eyre::Result;

pub fn init_static(disable_up_arrow: bool, disable_ctrl_r: bool) {
    let base = include_str!("../../../shell/atuin.fish");

    println!("{base}");

    if std::env::var("ATUIN_NOBIND").is_err() {
        const BIND_CTRL_R: &str = r"bind \cr _atuin_search";
        const BIND_UP_ARROW: &str = r"bind -k up _atuin_bind_up
bind \eOA _atuin_bind_up
bind \e\[A _atuin_bind_up";
        const BIND_CTRL_R_INS: &str = r"bind -M insert \cr _atuin_search";
        const BIND_UP_ARROW_INS: &str = r"bind -M insert -k up _atuin_bind_up
bind -M insert \eOA _atuin_bind_up
bind -M insert \e\[A _atuin_bind_up";

        if !disable_ctrl_r {
            println!("{BIND_CTRL_R}");
        }
        if !disable_up_arrow {
            println!("{BIND_UP_ARROW}");
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

pub async fn init(store: AliasStore, disable_up_arrow: bool, disable_ctrl_r: bool) -> Result<()> {
    init_static(disable_up_arrow, disable_ctrl_r);

    let aliases = atuin_dotfiles::shell::fish::config(&store).await;

    println!("{aliases}");

    Ok(())
}
