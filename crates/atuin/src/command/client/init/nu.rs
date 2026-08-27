use super::StaticInitOptions;

const BIND_CTRL_R: &str = r"$env.config = (
    $env.config | upsert keybindings (
        $env.config.keybindings
        | append {
            name: atuin
            modifier: control
            keycode: char_r
            mode: [emacs, vi_normal, vi_insert]
            event: { send: executehostcommand cmd: (_atuin_search_cmd) }
        }
    )
)";

const BIND_UP_ARROW: &str = r"$env.config = (
    $env.config | upsert keybindings (
        $env.config.keybindings
        | append {
            name: atuin_up_arrow
            modifier: none
            keycode: up
            mode: [emacs, vi_normal, vi_insert]
            event: {
                until: [
                    {send: menuup}
                    {send: executehostcommand cmd: (_atuin_search_cmd '--shell-up-key-binding') }
                ]
            }
        }
    )
)";

pub fn init_static(options: &StaticInitOptions<'_>) {
    // TODO: tmux popup for Nu
    println!("{}", crate::shell::NU);

    if std::env::var("ATUIN_NOBIND").is_err() {
        if options.enable_ctrl_r {
            println!("{BIND_CTRL_R}");
        }
        if options.enable_up_arrow {
            println!("{BIND_UP_ARROW}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use rstest::rstest;

    use super::*;

    #[rstest]
    fn keybindings_have_unique_names() {
        let re = regex::Regex::new(r"\<name:\s*(\w+)").unwrap();
        let mut names = HashSet::new();
        for name in [BIND_CTRL_R, BIND_UP_ARROW]
            .into_iter()
            .map(|s| re.captures(s).unwrap().get(1).unwrap().as_str().to_owned())
        {
            if let Some(existing) = names.replace(name) {
                panic!("duplicate binding name: {existing}");
            }
        }
    }
}
