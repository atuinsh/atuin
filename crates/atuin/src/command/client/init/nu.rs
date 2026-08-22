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
    use rstest::rstest;

    use super::{BIND_CTRL_R, BIND_UP_ARROW};

    #[rstest]
    #[case(BIND_CTRL_R, "atuin")]
    #[case(BIND_UP_ARROW, "atuin_up_arrow")]
    fn keybinding_has_unique_name(#[case] binding: &str, #[case] expected_name: &str) {
        assert!(binding.contains(&format!("name: {expected_name}")));
    }
}
