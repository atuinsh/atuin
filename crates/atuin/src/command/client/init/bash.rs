use std::io::{self, Write};

use atuin_dotfiles::store::AliasStore;
use atuin_dotfiles::store::var::VarStore;
use eyre::Result;

use super::StaticInitOptions;
use crate::shell::BASH;

fn write_popup_config<W: Write>(
    writer: &mut W,
    enabled: bool,
    width: &str,
    height: &str,
) -> io::Result<()> {
    // Keep the configured size available when popup mode is toggled on at runtime.
    writeln!(writer, "export ATUIN_POPUP_ENABLED={enabled}")?;
    writeln!(writer, "export ATUIN_POPUP_WIDTH='{width}'")?;
    writeln!(writer, "export ATUIN_POPUP_HEIGHT='{height}'")
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

    write_popup_config(writer, options.popup.enabled, &options.popup.width, &options.popup.height)?;
    writeln!(writer, "__atuin_bind_ctrl_r={bind_ctrl_r}")?;
    writeln!(writer, "__atuin_bind_up_arrow={bind_up_arrow}")?;
    writeln!(writer, "{}", BASH.main)?;

    #[cfg(feature = "ai")]
    if options.enable_ai {
        writeln!(writer, "{}", atuin_ai::shell::BASH_INIT)?;
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

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::write_popup_config;

    #[rstest]
    #[case::enabled(
        true,
        "70%",
        "50%",
        concat!(
            "export ATUIN_POPUP_ENABLED=true\n",
            "export ATUIN_POPUP_WIDTH='70%'\n",
            "export ATUIN_POPUP_HEIGHT='50%'\n",
        )
    )]
    #[case::disabled(
        false,
        "80%",
        "60%",
        concat!(
            "export ATUIN_POPUP_ENABLED=false\n",
            "export ATUIN_POPUP_WIDTH='80%'\n",
            "export ATUIN_POPUP_HEIGHT='60%'\n",
        )
    )]
    fn popup_config_exports_all_values(
        #[case] enabled: bool,
        #[case] width: &str,
        #[case] height: &str,
        #[case] expected: &str,
    ) {
        let mut output = Vec::new();

        write_popup_config(&mut output, enabled, width, height).unwrap();

        assert_eq!(String::from_utf8(output).unwrap(), expected);
    }
}
