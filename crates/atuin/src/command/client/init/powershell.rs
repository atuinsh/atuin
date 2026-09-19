use super::StaticInitOptions;

fn ps_bool(value: bool) -> &'static str {
    if value {
        "$true"
    } else {
        "$false"
    }
}

pub fn init_static(options: &StaticInitOptions<'_>) {
    let (bind_ctrl_r, bind_up_arrow) = if std::env::var("ATUIN_NOBIND").is_ok() {
        (false, false)
    } else {
        (options.enable_ctrl_r, options.enable_up_arrow)
    };

    // TODO: tmux popup for Powershell
    println!("{}", crate::shell::POWERSHELL);
    println!(
        "Enable-AtuinSearchKeys -CtrlR {} -UpArrow {}",
        ps_bool(bind_ctrl_r),
        ps_bool(bind_up_arrow)
    );
}
