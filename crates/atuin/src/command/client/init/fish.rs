use atuin_client::settings::Tmux;

use super::StaticInitOptions;

fn print_tmux_config(tmux: &Tmux) {
    if tmux.enabled {
        println!("set -gx ATUIN_TMUX_POPUP_WIDTH '{}'", tmux.width);
        println!("set -gx ATUIN_TMUX_POPUP_HEIGHT '{}'", tmux.height);
    } else {
        println!("set -gx ATUIN_TMUX_POPUP false");
    }
}

fn print_bindings(
    indent: &str,
    options: &StaticInitOptions<'_>,
    bind_ctrl_r: &str,
    bind_up_arrow: &str,
    bind_ctrl_r_ins: &str,
    bind_up_arrow_ins: &str,
) {
    if options.enable_ctrl_r {
        println!("{indent}{bind_ctrl_r}");
        println!("{indent}{bind_ctrl_r_ins}");
    }
    if options.enable_up_arrow {
        println!("{indent}{bind_up_arrow}");
        println!("{indent}{bind_up_arrow_ins}");
    }
}

/// ctrl-] opens `atuin ai resume`, in the default and insert modes.
fn print_ai_resume_bindings(indent: &str, options: &StaticInitOptions<'_>, key: &str) {
    if cfg!(feature = "ai") && options.enable_ai_resume {
        println!("{indent}bind {key} _atuin_ai_resume");
        println!("{indent}bind -M insert {key} _atuin_ai_resume");
    }
}

pub fn init_static(options: &StaticInitOptions<'_>) {
    let indent = " ".repeat(4);

    print_tmux_config(options.tmux);
    println!("{}", crate::shell::FISH);

    if std::env::var("ATUIN_NOBIND").is_err() {
        println!("if string match -q '4.*' $version");

        // In fish 4.0 and above the option bind -k doesn't exist anymore,
        // instead we can use key names and modifiers directly.
        print_bindings(
            &indent,
            options,
            "bind ctrl-r _atuin_search",
            "bind up _atuin_bind_up",
            "bind -M insert ctrl-r _atuin_search",
            "bind -M insert up _atuin_bind_up",
        );
        print_ai_resume_bindings(&indent, options, "ctrl-]");

        println!("else");

        // We keep these for compatibility with fish 3.x
        print_bindings(
            &indent,
            options,
            r"bind \cr _atuin_search",
            &[
                r"bind -k up _atuin_bind_up",
                r"bind \eOA _atuin_bind_up",
                r"bind \e\[A _atuin_bind_up",
            ]
            .join("; "),
            r"bind -M insert \cr _atuin_search",
            &[
                r"bind -M insert -k up _atuin_bind_up",
                r"bind -M insert \eOA _atuin_bind_up",
                r"bind -M insert \e\[A _atuin_bind_up",
            ]
            .join("; "),
        );
        print_ai_resume_bindings(&indent, options, r"\c]");

        println!("end");

        #[cfg(feature = "ai")]
        if options.enable_ai {
            println!("{}", atuin_ai::shell::FISH_INIT);
        }
    }
}
