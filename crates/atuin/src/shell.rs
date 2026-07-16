macro_rules! include_shell {
    ($path:literal) => {
        include_str!(concat!("shell/", $path)).trim_ascii_end()
    };
}

pub struct Bash<'a> {
    pub include_guard_start: &'a str,
    pub main: &'a str,
    pub include_guard_end: &'a str,
}

pub const BASH: Bash<'_> = Bash {
    include_guard_start: include_shell!("atuin.bash.d/01-include-guard-start.bash"),
    main: include_shell!("atuin.bash.d/02-main.bash"),
    include_guard_end: include_shell!("atuin.bash.d/03-include-guard-end.bash"),
};

pub const FISH: &str = include_shell!("atuin.fish");
pub const NU: &str = include_shell!("atuin.nu");
pub const POWERSHELL: &str = include_shell!("atuin.ps1");
pub const XONSH: &str = include_shell!("atuin.xsh");
pub const ZSH: &str = include_shell!("atuin.zsh");
