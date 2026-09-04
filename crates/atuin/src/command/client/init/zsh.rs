use atuin_dotfiles::store::AliasStore;
use atuin_dotfiles::store::var::VarStore;
use eyre::Result;

use super::StaticInitOptions;

fn print_popup_config(enabled: bool, width: &str, height: &str) {
    // Keep the configured size available when popup mode is toggled on at runtime.
    println!("export ATUIN_POPUP_ENABLED={enabled}");
    println!("export ATUIN_POPUP_WIDTH='{width}'");
    println!("export ATUIN_POPUP_HEIGHT='{height}'");
}

pub fn init_static(options: &StaticInitOptions<'_>) {
    print_popup_config(options.popup.enabled, &options.popup.width, &options.popup.height);
    println!("{}", crate::shell::ZSH);

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

        if options.enable_ctrl_r {
            println!("{BIND_CTRL_R}");
        }
        if options.enable_up_arrow {
            println!("{BIND_UP_ARROW}");
        }

        #[cfg(feature = "ai")]
        if options.enable_ai {
            println!("{}", atuin_ai::shell::ZSH_INIT);
        }
    }
}

pub async fn init(
    aliases: AliasStore,
    vars: VarStore,
    options: &StaticInitOptions<'_>,
) -> Result<()> {
    init_static(options);

    let aliases = atuin_dotfiles::shell::zsh::alias_config(&aliases).await;
    let vars = atuin_dotfiles::shell::zsh::var_config(&vars).await;

    println!("{aliases}");
    println!("{vars}");

    Ok(())
}
