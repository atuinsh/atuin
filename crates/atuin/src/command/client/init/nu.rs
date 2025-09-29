use eyre::Result;

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
const BIND_UP_ARROW: &str = r"
$env.config = (
    $env.config | upsert keybindings (
        $env.config.keybindings
        | append {
            name: atuin
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
)
";

pub fn init_static(disable_up_arrow: bool, disable_ctrl_r: bool) {
    let full = include_str!("../../../shell/atuin.nu");
    println!("{full}");

    if !disable_ctrl_r && std::env::var("ATUIN_NOBIND").is_err() {
        println!("{BIND_CTRL_R}");
    }
    if !disable_up_arrow && std::env::var("ATUIN_NOBIND").is_err() {
        println!("{BIND_UP_ARROW}");
    }
}

pub async fn init(disable_up_arrow: bool, disable_ctrl_r: bool) -> Result<()> {
    init_static(disable_up_arrow, disable_ctrl_r);

    let vars = atuin_dotfiles::shell::nu::var_config().await;

    println!("{vars}");

    Ok(())
}
