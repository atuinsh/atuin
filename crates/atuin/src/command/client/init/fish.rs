use atuin_dotfiles::store::{AliasStore, var::VarStore};
use eyre::Result;

fn print_bindings(
    indent: &str,
    disable_up_arrow: bool,
    disable_ctrl_r: bool,
    bind_ctrl_r: &str,
    bind_up_arrow: &str,
    bind_ctrl_r_ins: &str,
    bind_up_arrow_ins: &str,
) {
    if !disable_ctrl_r {
        println!("{indent}{bind_ctrl_r}");
    }
    if !disable_up_arrow {
        println!("{indent}{bind_up_arrow}");
    }

    println!("{indent}if bind -M insert >/dev/null 2>&1");
    if !disable_ctrl_r {
        println!("{indent}{indent}{bind_ctrl_r_ins}");
    }
    if !disable_up_arrow {
        println!("{indent}{indent}{bind_up_arrow_ins}");
    }
    println!("{indent}end");
}

pub fn init_static(disable_up_arrow: bool, disable_ctrl_r: bool) {
    let indent = " ".repeat(4);

    let base = include_str!("../../../shell/atuin.fish");

    println!("{base}");

    if std::env::var("ATUIN_NOBIND").is_err() {
        println!("if string match -q '4.*' $version");

        // In fish 4.0 and above the option bind -k doesn't exist anymore,
        // instead we can use key names and modifiers directly.
        print_bindings(
            &indent,
            disable_up_arrow,
            disable_ctrl_r,
            "bind ctrl-r _atuin_search",
            "bind up _atuin_bind_up",
            "bind -M insert ctrl-r _atuin_search",
            "bind -M insert up _atuin_bind_up",
        );

        println!("else");

        // We keep these for compatibility with fish 3.x
        print_bindings(
            &indent,
            disable_up_arrow,
            disable_ctrl_r,
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

        println!("end");
    }
}

pub async fn init(
    aliases: AliasStore,
    vars: VarStore,
    disable_up_arrow: bool,
    disable_ctrl_r: bool,
) -> Result<()> {
    init_static(disable_up_arrow, disable_ctrl_r);

    let aliases = atuin_dotfiles::shell::fish::alias_config(&aliases).await;
    let vars = atuin_dotfiles::shell::fish::var_config(&vars).await;

    println!("{aliases}");
    println!("{vars}");

    Ok(())
}
