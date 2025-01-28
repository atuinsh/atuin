pub fn init_static(disable_up_arrow: bool, disable_ctrl_r: bool) {
    let base = include_str!("../../../shell/atuin.ps1");

    let (bind_ctrl_r, bind_up_arrow) = if std::env::var("ATUIN_NOBIND").is_ok() {
        (false, false)
    } else {
        (!disable_ctrl_r, !disable_up_arrow)
    };

    println!("{base}");
    println!(
        "Enable-AtuinSearchKeys -CtrlR {} -UpArrow {}",
        ps_bool(bind_ctrl_r),
        ps_bool(bind_up_arrow)
    );
}

fn ps_bool(value: bool) -> &'static str {
    if value {
        "$true"
    } else {
        "$false"
    }
}
