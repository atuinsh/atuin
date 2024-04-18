use atuin_dotfiles::store::AliasStore;
use eyre::Result;

pub fn init_static(disable_up_arrow: bool, disable_ctrl_r: bool) {
    let base = include_str!("../../../shell/atuin.zsh");

    println!("{base}");

    if std::env::var("ATUIN_NOBIND").is_err() {
        const BIND_CTRL_R: &str = r"bindkey -M emacs '^r' atuin-search
bindkey -M viins '^r' atuin-search-viins
bindkey -M vicmd '/' atuin-search";

        const BIND_UP_ARROW: &str = r"bindkey -M emacs '^[[A' atuin-up-search
bindkey -M vicmd '^[[A' atuin-up-search-vicmd
bindkey -M viins '^[[A' atuin-up-search-viins
bindkey -M emacs '^[OA' atuin-up-search
bindkey -M vicmd '^[OA' atuin-up-search-vicmd
bindkey -M viins '^[OA' atuin-up-search-viins
bindkey -M vicmd 'k' atuin-up-search-vicmd";

        if !disable_ctrl_r {
            println!("{BIND_CTRL_R}");
        }
        if !disable_up_arrow {
            println!("{BIND_UP_ARROW}");
        }
    }
}

pub async fn init(store: AliasStore, disable_up_arrow: bool, disable_ctrl_r: bool) -> Result<()> {
    init_static(disable_up_arrow, disable_ctrl_r);

    let aliases = atuin_dotfiles::shell::zsh::config(&store).await;

    println!("{aliases}");

    Ok(())
}
