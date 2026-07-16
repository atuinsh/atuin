macro_rules! include_shell {
    ($path:literal) => {
        include_str!(concat!("shell/", $path)).trim_ascii_end()
    };
}

pub struct Bash<'a> {
    pub include_guard: &'a str,
    pub main: &'a str,
}

pub const BASH: Bash<'_> = Bash {
    include_guard: include_shell!("atuin.bash.d/include-guard.bash"),
    main: include_shell!("atuin.bash"),
};

pub const FISH: &str = include_shell!("atuin.fish");
pub const NU: &str = include_shell!("atuin.nu");
pub const POWERSHELL: &str = include_shell!("atuin.ps1");
pub const XONSH: &str = include_shell!("atuin.xsh");
pub const ZSH: &str = include_shell!("atuin.zsh");
