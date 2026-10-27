use std::io::{self, Write};

use atuin_client::settings::Tmux;

use super::StaticInitOptions;

fn print_tmux_config(writer: &mut impl Write, tmux: &Tmux) -> io::Result<()> {
    if tmux.enabled {
        writeln!(writer, "set -gx ATUIN_TMUX_POPUP_WIDTH '{}'", tmux.width)?;
        writeln!(writer, "set -gx ATUIN_TMUX_POPUP_HEIGHT '{}'", tmux.height)
    } else {
        writeln!(writer, "set -gx ATUIN_TMUX_POPUP false")
    }
}

fn print_bindings(
    writer: &mut impl Write,
    indent: &str,
    options: &StaticInitOptions<'_>,
    bind_ctrl_r: &str,
    bind_up_arrow: &str,
    bind_ctrl_r_ins: &str,
    bind_up_arrow_ins: &str,
) -> io::Result<()> {
    if options.enable_ctrl_r {
        writeln!(writer, "{indent}{bind_ctrl_r}")?;
        writeln!(writer, "{indent}{bind_ctrl_r_ins}")?;
    }
    if options.enable_up_arrow {
        writeln!(writer, "{indent}{bind_up_arrow}")?;
        writeln!(writer, "{indent}{bind_up_arrow_ins}")?;
    }
    Ok(())
}

/// ctrl-] opens `atuin threads resume`, in the default and insert modes.
fn print_ai_resume_bindings(
    writer: &mut impl Write,
    indent: &str,
    options: &StaticInitOptions<'_>,
    key: &str,
) -> io::Result<()> {
    if cfg!(feature = "ai") && options.enable_ai_resume {
        writeln!(writer, "{indent}bind {key} _atuin_ai_resume")?;
        writeln!(writer, "{indent}bind -M insert {key} _atuin_ai_resume")?;
    }
    Ok(())
}

pub fn init_static(options: &StaticInitOptions<'_>) -> io::Result<()> {
    let indent = " ".repeat(4);
    let mut stdout = io::stdout().lock();

    print_tmux_config(&mut stdout, options.tmux)?;
    writeln!(stdout, "{}", crate::shell::FISH)?;

    if atuin_common::env::var("ATUIN_NOBIND").is_err() {
        writeln!(stdout, "if string match -q '4.*' $version")?;

        // In fish 4.0 and above the option bind -k doesn't exist anymore,
        // instead we can use key names and modifiers directly.
        print_bindings(
            &mut stdout,
            &indent,
            options,
            "bind ctrl-r _atuin_search",
            "bind up _atuin_bind_up",
            "bind -M insert ctrl-r _atuin_search",
            "bind -M insert up _atuin_bind_up",
        )?;
        print_ai_resume_bindings(&mut stdout, &indent, options, "ctrl-]")?;

        writeln!(stdout, "else")?;

        // We keep these for compatibility with fish 3.x
        print_bindings(
            &mut stdout,
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
        )?;
        print_ai_resume_bindings(&mut stdout, &indent, options, r"\c]")?;

        writeln!(stdout, "end")?;

        #[cfg(feature = "ai")]
        if options.enable_ai {
            writeln!(stdout, "{}", atuin_ai::shell::FISH_INIT)?;
        }
    }

    Ok(())
}
