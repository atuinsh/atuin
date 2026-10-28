use atuin_client::settings::Tmux;

use super::StaticInitOptions;

fn print_tmux_config(tmux: &Tmux) {
    if tmux.enabled {
        println!("export ATUIN_TMUX_POPUP_WIDTH='{}'", tmux.width);
        println!("export ATUIN_TMUX_POPUP_HEIGHT='{}'", tmux.height);
    } else {
        println!("export ATUIN_TMUX_POPUP=false");
    }
}

pub fn init_static(options: &StaticInitOptions<'_>) {
    print_tmux_config(options.tmux);
    println!("{}", crate::shell::ZSH);

    if atuin_common::env::var("ATUIN_NOBIND").is_err() {
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

        #[cfg(feature = "si")]
        if options.enable_si {
            println!("{}", atuin_si::shell::ZSH_INIT);
        }

        #[cfg(feature = "si")]
        if options.enable_si_resume {
            println!(
                r"bindkey -M emacs '^]' atuin-si-resume
bindkey -M viins '^]' atuin-si-resume-viins
bindkey -M vicmd '^]' atuin-si-resume-vicmd"
            );
        }
    }
}
