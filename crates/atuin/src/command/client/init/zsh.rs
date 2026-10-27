use std::io::{self, Write};

use atuin_client::settings::Tmux;

use super::StaticInitOptions;

fn print_tmux_config(writer: &mut impl Write, tmux: &Tmux) -> io::Result<()> {
    if tmux.enabled {
        writeln!(writer, "export ATUIN_TMUX_POPUP_WIDTH='{}'", tmux.width)?;
        writeln!(writer, "export ATUIN_TMUX_POPUP_HEIGHT='{}'", tmux.height)
    } else {
        writeln!(writer, "export ATUIN_TMUX_POPUP=false")
    }
}

pub fn init_static(options: &StaticInitOptions<'_>) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    print_tmux_config(&mut stdout, options.tmux)?;
    writeln!(stdout, "{}", crate::shell::ZSH)?;

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
            writeln!(stdout, "{BIND_CTRL_R}")?;
        }
        if options.enable_up_arrow {
            writeln!(stdout, "{BIND_UP_ARROW}")?;
        }

        #[cfg(feature = "ai")]
        if options.enable_ai {
            writeln!(stdout, "{}", atuin_ai::shell::ZSH_INIT)?;
        }

        #[cfg(feature = "ai")]
        if options.enable_ai_resume {
            writeln!(
                stdout,
                r"bindkey -M emacs '^]' atuin-ai-resume
bindkey -M viins '^]' atuin-ai-resume-viins
bindkey -M vicmd '^]' atuin-ai-resume-vicmd"
            )?;
        }
    }

    Ok(())
}
