macro_rules! include_trimmed {
    ($path:expr) => {
        include_str!($path).trim_ascii_end()
    };
}

macro_rules! include_shell {
    ($path:expr) => {
        include_trimmed!(concat!("shell/", $path))
    };
}

pub struct Bash<'a> {
    pub include_guard: &'a str,
    pub main: &'a str,
    pub preexec: &'a str,
}

pub const BASH: Bash<'_> = Bash {
    include_guard: include_shell!("atuin.bash.d/include-guard.bash"),
    main: include_shell!("atuin.bash"),
    preexec: include_trimmed!("../../../vendor/bash-preexec/bash-preexec.sh"),
};

pub const FISH: &str = include_shell!("atuin.fish");
pub const NU: &str = include_shell!("atuin.nu");
pub const POWERSHELL: &str = include_shell!("atuin.ps1");
pub const XONSH: &str = include_shell!("atuin.xsh");
pub const ZSH: &str = include_shell!("atuin.zsh");
