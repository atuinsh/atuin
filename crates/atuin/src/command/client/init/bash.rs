use super::StaticInitOptions;
use crate::shell::BASH;
use atuin_client::settings::Tmux;
use atuin_dotfiles::store::{AliasStore, var::VarStore};
use eyre::Result;
use std::io::{self, Write};

fn write_tmux_config<W: Write>(writer: &mut W, tmux: &Tmux) -> io::Result<()> {
    if tmux.enabled {
        writeln!(writer, "export ATUIN_TMUX_POPUP_WIDTH='{}'", tmux.width)?;
        writeln!(writer, "export ATUIN_TMUX_POPUP_HEIGHT='{}'", tmux.height)
    } else {
        writeln!(writer, "export ATUIN_TMUX_POPUP=false")
    }
}

fn write_static_init<W: Write>(writer: &mut W, options: &StaticInitOptions<'_>) -> io::Result<()> {
    let (bind_ctrl_r, bind_up_arrow) = if std::env::var("ATUIN_NOBIND").is_ok() {
        (false, false)
    } else {
        (options.enable_ctrl_r, options.enable_up_arrow)
    };

    writeln!(writer, "{} && {{", BASH.include_guard)?;

    if std::env::var_os("ATUIN_NO_BUILTIN_PREEXEC").is_none_or(|s| s.is_empty()) {
        writeln!(writer, "# Set ATUIN_NO_BUILTIN_PREEXEC=1 to disable loading bash-preexec")?;
        writeln!(writer, "__atuin_load_builtin_preexec() {{")?;
        for line in BASH.preexec.lines() {
            writeln!(writer, "    {line}")?;
        }
        writeln!(writer, "}}")?;
    }

    write_tmux_config(writer, options.tmux)?;
    writeln!(writer, "__atuin_bind_ctrl_r={bind_ctrl_r}")?;
    writeln!(writer, "__atuin_bind_up_arrow={bind_up_arrow}")?;
    writeln!(writer, "{}", BASH.main)?;

    #[cfg(feature = "ai")]
    if options.enable_ai {
        let bind_ai = atuin_ai::commands::init::generate_bash_integration();
        writeln!(writer, "{bind_ai}")?;
    }

    writeln!(writer, "}}") // end include guard
}

pub fn init_static(options: &StaticInitOptions<'_>) {
    if let Err(e) = write_static_init(&mut io::stdout().lock(), options) {
        // This function used to use `println!`, which panics on write failure with this same
        // message. Using a locked `Stdout` object is faster, but `write!` returns an error rather
        // than panicking, so we manually panic here to keep the same behavior.
        panic!("failed printing to stdout: {e}");
    }
}

pub async fn init(
    aliases: AliasStore,
    vars: VarStore,
    options: &StaticInitOptions<'_>,
) -> Result<()> {
    init_static(options);

    let aliases = atuin_dotfiles::shell::bash::alias_config(&aliases).await;
    let vars = atuin_dotfiles::shell::bash::var_config(&vars).await;

    println!("{aliases}");
    println!("{vars}");

    Ok(())
}
