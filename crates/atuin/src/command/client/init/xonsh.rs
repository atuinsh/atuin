use std::io::{self, Write as _};

use super::StaticInitOptions;

pub fn init_static(options: &StaticInitOptions<'_>) -> io::Result<()> {
    let (bind_ctrl_r, bind_up_arrow) = if atuin_common::env::var("ATUIN_NOBIND").is_ok() {
        (false, false)
    } else {
        (options.enable_ctrl_r, options.enable_up_arrow)
    };

    let mut stdout = io::stdout().lock();
    // TODO: tmux popup for xonsh
    writeln!(
        stdout,
        "_ATUIN_BIND_CTRL_R={}",
        if bind_ctrl_r {
            "True"
        } else {
            "False"
        }
    )?;
    writeln!(
        stdout,
        "_ATUIN_BIND_UP_ARROW={}",
        if bind_up_arrow {
            "True"
        } else {
            "False"
        }
    )?;
    writeln!(stdout, "{}", crate::shell::XONSH)
}
