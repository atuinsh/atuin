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

#[cfg(test)]
mod tests {
    use rstest::rstest;

    #[rstest]
    fn failed_search_preserves_the_xonsh_buffer() {
        let script = crate::shell::XONSH;
        let status_check = script.find("if p.returncode != 0:").unwrap();
        let buffer_reset = script.find("buffer.reset()").unwrap();

        assert!(status_check < buffer_reset);
        assert!(script.contains(
            "if p.returncode != 0:\n        if result:\n            print(result, \
             file=sys.stderr)\n        return"
        ));
    }
}
