use super::StaticInitOptions;

pub fn init_static(options: &StaticInitOptions<'_>) {
    let (bind_ctrl_r, bind_up_arrow) = if std::env::var("ATUIN_NOBIND").is_ok() {
        (false, false)
    } else {
        (options.enable_ctrl_r, options.enable_up_arrow)
    };

    // TODO: tmux popup for xonsh
    println!(
        "_ATUIN_BIND_CTRL_R={}",
        if bind_ctrl_r {
            "True"
        } else {
            "False"
        }
    );
    println!(
        "_ATUIN_BIND_UP_ARROW={}",
        if bind_up_arrow {
            "True"
        } else {
            "False"
        }
    );
    println!("{}", crate::shell::XONSH);
}
