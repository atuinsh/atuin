use clap::{Parser, ValueEnum};

#[derive(Parser)]
pub struct Cmd {
    shell: Shell,

    /// Disable the binding of CTRL-R to atuin
    #[clap(long)]
    disable_ctrl_r: bool,

    /// Disable the binding of the Up Arrow key to atuin
    #[clap(long)]
    disable_up_arrow: bool,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Shell {
    /// Zsh setup
    Zsh,
    /// Bash setup
    Bash,
    /// Fish setup
    Fish,
    /// Nu setup
    Nu,
}

impl Cmd {
    fn init_zsh(&self) {
        let base = include_str!("../shell/atuin.zsh");

        println!("{base}");

        if std::env::var("ATUIN_NOBIND").is_err() {
            const BIND_CTRL_R: &str = r"bindkey -M emacs '^r' atuin-search
bindkey -M viins '^r' atuin-search-viins
bindkey -M vicmd '/' atuin-search";

            const BIND_UP_ARROW: &str = r"bindkey -M emacs '^[[A' atuin-up-search
bindkey -M vicmd '^[[A' atuin-up-search-vicmd
bindkey -M viins '^[[A' atuin-up-search-viins
bindkey -M emacs '^[OA' atuin-up-search
bindkey -M vicmd '^[OA' atuin-up-search-vicmd
bindkey -M viins '^[OA' atuin-up-search-viins
bindkey -M vicmd 'k' atuin-up-search-vicmd";

            if !self.disable_ctrl_r {
                println!("{BIND_CTRL_R}");
            }
            if !self.disable_up_arrow {
                println!("{BIND_UP_ARROW}");
            }
        }
    }

    fn init_bash(&self) {
        let base = include_str!("../shell/atuin.bash");
        let (bind_ctrl_r, bind_up_arrow) = if std::env::var("ATUIN_NOBIND").is_ok() {
            (false, false)
        } else {
            (!self.disable_ctrl_r, !self.disable_up_arrow)
        };

        println!("__atuin_bind_ctrl_r={bind_ctrl_r}");
        println!("__atuin_bind_up_arrow={bind_up_arrow}");
        println!("{base}");
    }

    fn init_fish(&self) {
        let full = include_str!("../shell/atuin.fish");
        println!("{full}");

        if std::env::var("ATUIN_NOBIND").is_err() {
            const BIND_CTRL_R: &str = r"bind \cr _atuin_search";
            const BIND_UP_ARROW: &str = r"bind -k up _atuin_bind_up
bind \eOA _atuin_bind_up
bind \e\[A _atuin_bind_up";
            const BIND_CTRL_R_INS: &str = r"bind -M insert \cr _atuin_search";
            const BIND_UP_ARROW_INS: &str = r"bind -M insert -k up _atuin_bind_up
bind -M insert \eOA _atuin_bind_up
bind -M insert \e\[A _atuin_bind_up";

            if !self.disable_ctrl_r {
                println!("{BIND_CTRL_R}");
            }
            if !self.disable_up_arrow {
                println!("{BIND_UP_ARROW}");
            }

            println!("if bind -M insert > /dev/null 2>&1");
            if !self.disable_ctrl_r {
                println!("{BIND_CTRL_R_INS}");
            }
            if !self.disable_up_arrow {
                println!("{BIND_UP_ARROW_INS}");
            }
            println!("end");
        }
    }

    fn init_nu(&self) {
        let full = include_str!("../shell/atuin.nu");
        println!("{full}");

        if std::env::var("ATUIN_NOBIND").is_err() {
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
# The up arrow keybinding has surprising behavior in Nu, and is disabled by default.
# See https://github.com/atuinsh/atuin/issues/1025 for details
# $env.config = (
#     $env.config | upsert keybindings (
#         $env.config.keybindings
#         | append {
#             name: atuin
#             modifier: none
#             keycode: up
#             mode: [emacs, vi_normal, vi_insert]
#             event: { send: executehostcommand cmd: (_atuin_search_cmd '--shell-up-key-binding') }
#         }
#     )
# )
";
            if !self.disable_ctrl_r {
                println!("{BIND_CTRL_R}");
            }
            if !self.disable_up_arrow {
                println!("{BIND_UP_ARROW}");
            }
        }
    }

    pub fn run(self) {
        match self.shell {
            Shell::Zsh => self.init_zsh(),
            Shell::Bash => self.init_bash(),
            Shell::Fish => self.init_fish(),
            Shell::Nu => self.init_nu(),
        }
    }
}
