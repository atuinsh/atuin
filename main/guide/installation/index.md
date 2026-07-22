# Installation

## Recommended installation approach

### On Unix

Let's get started! First up, you will want to install Atuin. The recommended approach is to use the installation script, which automatically handles the installation of Atuin including the requirements for your environment.

It will install a binary to `~/.atuin/bin`, and if you'd rather do something else then the manual steps below offer much more flexibility.

```
curl --proto '=https' --tlsv1.2 -LsSf https://setup.atuin.sh | sh
```

The install script will walk you through importing your shell history and setting up a sync account. To skip these interactive prompts (e.g. in CI or Dockerfiles), pass `--non-interactive`:

```
curl --proto '=https' --tlsv1.2 -LsSf https://setup.atuin.sh | sh -s -- --non-interactive
```

The script also automatically detects non-interactive environments (piped input, no TTY) and skips the prompts in those cases.

[**Set up sync** - Move on to the next step, or read on to manually install Atuin instead.](https://docs.atuin.sh/guide/sync/index.md)

### On Windows

The recommended approach on Windows is to use WinGet to install Atuin. Then, if you use PowerShell, add the initialization command to your PowerShell profile, and restart your shell.

```
winget install -e Atuinsh.Atuin
if (-not (Test-Path -Path $PROFILE)) { New-Item -ItemType File -Path $PROFILE -Force | Out-Null }
Write-Output 'atuin init powershell | Out-String | Invoke-Expression' >> $PROFILE
```

Note that the `$PROFILE` path may depend on your PowerShell version.

[**Set up sync** - Move on to the next step.](https://docs.atuin.sh/guide/sync/index.md)

## Manual installation

### Installing the binary

If you don't wish to use the installer, the manual installation steps are as follows.

It's best to use [rustup](https://rustup.rs/) to set up a Rust toolchain, then you can run:

```
cargo install atuin --locked
```

```
brew install atuin
```

Atuin is also available in [MacPorts](https://ports.macports.org/port/atuin/)

```
sudo port install atuin
```

Atuin is also installable using [mise](https://github.com/jdx/mise)

```
mise use -g atuin@latest
```

This repository is a flake, and can be installed using `nix profile`:

```
nix profile install "github:atuinsh/atuin"
```

Atuin is also available in [nixpkgs](https://github.com/NixOS/nixpkgs):

```
nix-env -f '<nixpkgs>' -iA atuin
```

Atuin is available in the Arch Linux [extra repository](https://archlinux.org/packages/extra/x86_64/atuin/):

```
pacman -S atuin
```

Atuin is available in the Void Linux [repository](https://github.com/void-linux/void-packages/tree/master/srcpkgs/atuin):

```
sudo xbps-install atuin
```

Atuin is available in the Termux package repository:

```
pkg install atuin
```

Atuin is installable from github-releases directly:

```
# line 1: `atuin` binary as command, from github release, only look at .tar.gz files, use the `atuin` file from the extracted archive
# line 2: setup at clone(create init.zsh, completion)
# line 3: pull behavior same as clone, source init.zsh
zinit ice as"command" from"gh-r" bpick"atuin-*.tar.gz" mv"atuin*/atuin -> atuin" \
    atclone"./atuin init zsh > init.zsh; ./atuin gen-completions --shell zsh > _atuin" \
    atpull"%atclone" src"init.zsh"
zinit light atuinsh/atuin
```

Atuin is available on WinGet:

```
winget install -e Atuinsh.Atuin
```

Atuin builds on the latest stable version of Rust, and we make no promises regarding older versions. We recommend using [rustup](https://rustup.rs/).

```
git clone https://github.com/atuinsh/atuin.git
cd atuin
cargo install --path crates/atuin --locked
```

Please be advised

If you choose to manually install Atuin rather than using the recommended installation script, merely installing the binary is not sufficient, you should also set up the shell plugin.

______________________________________________________________________

### Installing the shell plugin

Once the binary is installed, the shell plugin requires installing. If you use the install script, this should all be done for you! After installing, remember to restart your shell.

```
echo 'eval "$(atuin init zsh)"' >> ~/.zshrc
```

```
# if you _only_ want to install the shell-plugin, do this; otherwise look above for a "everything via zinit" solution
zinit load atuinsh/atuin
```

```
antigen bundle atuinsh/atuin@main
```

```
antidote install atuinsh/atuin
```

Atuin works best in bash when using [ble.sh](https://github.com/akinomyoga/ble.sh) >= 0.4.

With ble.sh (>= 0.4) installed and loaded in `~/.bashrc`, just add atuin to your `~/.bashrc`

```
echo 'eval "$(atuin init bash)"' >> ~/.bashrc
```

[bash-preexec](https://github.com/rcaloras/bash-preexec) can also be used, but you may experience some minor problems with the recorded duration and exit status of some commands.

Please note

bash-preexec currently has [an issue](https://github.com/rcaloras/bash-preexec/issues/115) where it will stop honoring `ignorespace`. While Atuin will ignore commands prefixed with whitespace, they may still end up in your bash history. Please check your configuration! All other shells do not have this issue.

To use `atuin < 18.10.0` in `bash < 4` with bash-preexec, the option `enter_accept` needs to be turned on (which is so by default). There is no restriction in the latest version of Atuin (>= 18.10.0).

bash-preexec cannot properly invoke the `preexec` hook for subshell commands `(...)`, function definitions `func() { ...; }`, empty for-in-statements `for i in; do ...; done`, etc., so those commands and duration may not be recorded in the Atuin's history correctly.

As of Atuin 18.18.0, `atuin init bash` will automatically load bash-preexec if no other preexec backend has been loaded (ble.sh or an external copy of bash-preexec). To disable this behavior, pass `ATUIN_NO_BUILTIN_PREEXEC=1` to `atuin init`, e.g.:

```
eval "$(ATUIN_NO_BUILTIN_PREEXEC=1 atuin init bash)"
```

If you prefer, you can also download and install bash-preexec separately:

```
curl https://raw.githubusercontent.com/rcaloras/bash-preexec/master/bash-preexec.sh -o ~/.bash-preexec.sh
echo '[[ -f ~/.bash-preexec.sh ]] && source ~/.bash-preexec.sh' >> ~/.bashrc
```

Then set up Atuin:

```
echo 'eval "$(atuin init bash)"' >> ~/.bashrc
```

Add

```
atuin init fish | source
```

to your `is-interactive` block in your `~/.config/fish/config.fish` file

Run in *Nushell*:

```
mkdir ~/.local/share/atuin/
atuin init nu | save ~/.local/share/atuin/init.nu
```

Add to `config.nu`:

```
source ~/.local/share/atuin/init.nu
```

Optional: Atuin pty-proxy

pty-proxy is a lightweight pty proxy that renders the Atuin popup over your previous output, restoring it when closed — no clearing, no fullscreen. To use pty-proxy with Nushell, generate the init script:

```
mkdir ~/.local/share/atuin/
atuin pty-proxy init nu | save -f ~/.local/share/atuin/pty-proxy-init.nu
```

Then source it as early as possible in your `config.nu`, *before* the regular atuin init:

```
source ~/.local/share/atuin/pty-proxy-init.nu
source ~/.local/share/atuin/init.nu
```

Nushell's `source` command requires a static file path, so you must pre-generate both files.

Add

```
execx($(atuin init xonsh))
```

to the end of your `~/.xonshrc`

Add the following to the end of your `$PROFILE` file:

```
atuin init powershell | Out-String | Invoke-Expression
```

## Upgrade

Run `atuin update`, and if that command is not available, run the install script again.

If you used a package manager to install Atuin, then you should also use your package manager to update Atuin.

## Uninstall

If you'd like to uninstall Atuin, please check out [the uninstall page](https://docs.atuin.sh/uninstall/index.md).
