# Installation

## Recommended installation approach

Let's get started! First up, you will want to install Atuin. The recommended
approach is to use the installation script, which automatically handles the
installation of Atuin including the requirements for your environment.


It will install a binary to `~/.atuin/bin`, and if you'd rather do something else
then the manual steps below offer much more flexibility.

```shell
curl --proto '=https' --tlsv1.2 -LsSf https://setup.atuin.sh | sh
```

[**Setup sync** - Move on to the next step, or read on to manually install Atuin instead.](sync.md)

## Manual installation

### Installing the binary

If you don't wish to use the installer, the manual installation steps are as follows.

=== "Cargo"

    It's best to use [rustup](https://rustup.rs/) to get setup with a Rust
    toolchain, then you can run:

    ```shell
    cargo install atuin
    ```

=== "Homebrew"

    ```shell
    brew install atuin
    ```

=== "MacPorts"

    Atuin is also available in [MacPorts](https://ports.macports.org/port/atuin/)

    ```shell
    sudo port install atuin
    ```

=== "Nix"

    This repository is a flake, and can be installed using `nix profile`:

    ```shell
    nix profile install "github:atuinsh/atuin"
    ```

    Atuin is also available in [nixpkgs](https://github.com/NixOS/nixpkgs):

    ```shell
    nix-env -f '<nixpkgs>' -iA atuin
    ```

=== "Pacman"

    Atuin is available in the Arch Linux [extra repository](https://archlinux.org/packages/extra/x86_64/atuin/):

    ```shell
    pacman -S atuin
    ```

=== "XBPS"

    Atuin is available in the Void Linux [repository](https://github.com/void-linux/void-packages/tree/master/srcpkgs/atuin):

    ```shell
    sudo xbps-install atuin
    ```

=== "Termux"

    Atuin is available in the Termux package repository:

    ```shell
    pkg install atuin
    ```

=== "zinit"

    Atuin is installable from github-releases directly:

    ```shell
    # line 1: `atuin` binary as command, from github release, only look at .tar.gz files, use the `atuin` file from the extracted archive
    # line 2: setup at clone(create init.zsh, completion)
    # line 3: pull behavior same as clone, source init.zsh
    zinit ice as"command" from"gh-r" bpick"atuin-*.tar.gz" mv"atuin*/atuin -> atuin" \
        atclone"./atuin init zsh > init.zsh; ./atuin gen-completions --shell zsh > _atuin" \
        atpull"%atclone" src"init.zsh"
    zinit light atuinsh/atuin
    ```

=== "Source"

    Atuin builds on the latest stable version of Rust, and we make no
    promises regarding older versions. We recommend using [rustup](https://rustup.rs/).

    ```shell
    git clone https://github.com/atuinsh/atuin.git
    cd atuin/crates/atuin
    cargo install --path .
    ```

!!! warning "Please be advised"

    If you choose to manually install Atuin rather than using the recommended installation script,
    merely installing the binary is not sufficient, you should also set up the shell plugin.

---

### Installing the shell plugin

Once the binary is installed, the shell plugin requires installing.
If you use the install script, this should all be done for you!
After installing, remember to restart your shell.

=== "zsh"

    ```shell
    echo 'eval "$(atuin init zsh)"' >> ~/.zshrc
    ```

    === "zinit"

        ```shell
        # if you _only_ want to install the shell-plugin, do this; otherwise look above for a "everything via zinit" solution
        zinit load atuinsh/atuin
        ```

    === "Antigen"

        ```shell
        antigen bundle atuinsh/atuin@main
        ```

=== "bash"

    === "ble.sh"

        Atuin works best in bash when using [ble.sh](https://github.com/akinomyoga/ble.sh) >= 0.4.

        With ble.sh (>= 0.4) installed and loaded in `~/.bashrc`, just add atuin to your `~/.bashrc`

        ```shell
        echo 'eval "$(atuin init bash)"' >> ~/.bashrc
        ```

    === "bash-preexec"

        [Bash-preexec](https://github.com/rcaloras/bash-preexec) can also be used, but you may experience
         some minor problems with the recorded duration and exit status of some commands.

        !!! warning "Please note"

            bash-preexec currently has an issue where it will stop honoring `ignorespace`.
            While Atuin will ignore commands prefixed with whitespace, they may still end up in your bash history.
            Please check your configuration! All other shells do not have this issue.

            To use Atuin in `bash < 4` with bash-preexec, the option `enter_accept` needs
            to be turned on (which is so by default).

            bash-preexec cannot properly invoke the `preexec` hook for subshell commands
            `(...)`, function definitions `func() { ...; }`, empty for-in-statements `for
            i in; do ...; done`, etc., so those commands and duration may not be recorded
            in the Atuin's history correctly.

        To use bash-preexec, download and initialize it

        ```shell
        curl https://raw.githubusercontent.com/rcaloras/bash-preexec/master/bash-preexec.sh -o ~/.bash-preexec.sh
        echo '[[ -f ~/.bash-preexec.sh ]] && source ~/.bash-preexec.sh' >> ~/.bashrc
        ```

        Then setup Atuin

        ```shell
        echo 'eval "$(atuin init bash)"' >> ~/.bashrc
        ```

=== "fish"

    Add

    ```shell
    atuin init fish | source
    ```

    to your `is-interactive` block in your `~/.config/fish/config.fish` file

=== "Nushell"

    Run in *Nushell*:

    ```shell
    mkdir ~/.local/share/atuin/
    atuin init nu | save ~/.local/share/atuin/init.nu
    ```

    Add to `config.nu`:

    ```shell
    source ~/.local/share/atuin/init.nu
    ```

=== "xonsh"

    Add
    ```shell
    execx($(atuin init xonsh))
    ```
    to the end of your `~/.xonshrc`

## Upgrade

Run `atuin update`, and if that command is not available, run the install script again.

If you used a package manager to install Atuin, then you should also use your package manager to update Atuin.

## Uninstall

If you'd like to uninstall Atuin, please check out [the uninstall page](../uninstall.md).
