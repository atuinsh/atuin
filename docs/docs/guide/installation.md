# Installation

## Recommended installation approach

### On Unix

Let's get started! First up, you will want to install Atuin. The recommended
approach is to use the installation script, which automatically handles the
installation of Atuin including the requirements for your environment.


It will install a binary to `~/.atuin/bin`, and if you'd rather do something else
then the manual steps below offer much more flexibility.

```shell
curl --proto '=https' --tlsv1.2 -LsSf https://setup.atuin.sh | sh
```

After installing, the script runs `atuin setup`. Setup hooks Atuin into your
shells, imports your shell history, and offers to set up a sync account. To skip
these interactive prompts (for example, in CI or Dockerfiles), pass
`--non-interactive`:

```shell
curl --proto '=https' --tlsv1.2 -LsSf https://setup.atuin.sh | sh -s -- --non-interactive
```

The script also automatically detects non-interactive environments (piped input,
no TTY) and skips the prompts in those cases. Without prompts, the script runs
`atuin setup --yes --no-import`, which sets Atuin up without asking and doesn't
import your history.

Anything else you pass goes to `atuin setup`, which starts from those choices. For
example, to leave the up arrow to your shell and keep everything on this machine:

```shell
curl --proto '=https' --tlsv1.2 -LsSf https://setup.atuin.sh | sh -s -- --no-up-arrow --sync off
```

Run `atuin setup --help` for every option. You can run `atuin setup` again at any
time to change what it set up.

If you skipped sync during setup, sign in to Atuin Cloud later with
`atuin setup --sync cloud`. See [Setting up sync](sync.md) for the details,
including how to use your own server.

[**Basic usage** - Move on to the next step, or read on to manually install Atuin instead.](basic-usage.md)

### On Windows

The recommended approach on Windows is to use WinGet to install Atuin. Then, in a new shell, run
`atuin setup`, and restart your shell once setup finishes.

```shell
winget install -e Atuinsh.Atuin
```

```shell
atuin setup
```

Setup adds Atuin to the profiles of both PowerShell 7 and Windows PowerShell.

!!! tip "Installing for every user"

    By default, WinGet installs Atuin for your user only, which doesn't need administrator rights.
    To install it for every user on the machine, add `--scope machine`:

    ```shell
    winget install -e Atuinsh.Atuin --scope machine
    ```

You may also want to try the `default-powershell` [theme](theming.md), which uses the default PowerShell syntax colors.

If you skipped sync during setup, sign in to Atuin Cloud later with `atuin setup --sync cloud`.

[**Basic usage** - Move on to the next step.](basic-usage.md)

## Manual installation

### Installing the binary

If you don't wish to use the installer, the manual installation steps are as follows.

=== "Cargo"

    Atuin requires a recent Rust toolchain. The minimum supported version is
    set by `rust-version` in the workspace
    [`Cargo.toml`](https://github.com/atuinsh/atuin/blob/main/Cargo.toml)
    (currently {{ msrv }}). Distribution package managers often ship an older Rust,
    so it's best to use [rustup](https://rustup.rs/) to set up an up-to-date
    toolchain, then you can run:

    ```shell
    cargo install atuin --locked
    ```

=== "Homebrew"

    ```shell
    brew install atuin
    ```

=== "MacPorts"

    Atuin is also available in [MacPorts](https://ports.macports.org/port/atuin/):

    ```shell
    sudo port install atuin
    ```

=== "mise"

    Atuin is also installable using [mise](https://github.com/jdx/mise):

    ```shell
    mise use -g atuin@latest
    ```

=== "Nix"

    This repository is a flake, and can be installed using `nix profile`:

    ```shell
    nix profile install "github:atuinsh/atuin"
    ```

    Linux builds of `main` are cached on [Cachix](https://atuin.cachix.org). Run `cachix use atuin` first to download them instead of building from source.

=== "Pacman"

    Atuin is available in the Arch Linux [extra repository](https://archlinux.org/packages/extra/x86_64/atuin/):

    ```shell
    pacman -S atuin
    ```

=== "Termux"

    Atuin is available in the Termux package repository:

    ```shell
    pkg install atuin
    ```

=== "zinit"

    Atuin is installable from github-releases directly:

    ```shell
    # line 1: `atuin` binary as command, from github release, only look at .tar.gz files (excluding server), use the `atuin` file from the extracted archive
    # line 2: setup at clone(create init.zsh, completion)
    # line 3: pull behavior same as clone, source init.zsh
    zinit ice as"command" from"gh-r" bpick"atuin-*.tar.gz~*server*" mv"atuin*/atuin -> atuin" \
        atclone"./atuin init zsh > init.zsh; ./atuin gen-completions --shell zsh > _atuin" \
        atpull"%atclone" src"init.zsh"
    zinit light atuinsh/atuin
    ```

=== "WinGet"

    Atuin is available on WinGet:

    ```shell
    winget install -e Atuinsh.Atuin
    ```

=== "Source"

    Atuin builds on the latest stable version of Rust, and we make no
    promises regarding older versions. The minimum supported version is set by
    `rust-version` in the workspace
    [`Cargo.toml`](https://github.com/atuinsh/atuin/blob/main/Cargo.toml)
    (currently {{ msrv }}). We recommend using [rustup](https://rustup.rs/).

    ```shell
    git clone https://github.com/atuinsh/atuin.git
    cd atuin
    cargo install --path crates/atuin --locked
    ```

!!! warning "Please be advised"

    After installing the binary by hand, run `atuin setup` to hook Atuin into your shells:

    ```shell
    atuin setup
    ```

    Setup also imports your shell history and offers to set up sync. For Nushell, xonsh, and zsh
    plugin managers, see [Installing the shell plugin](#installing-the-shell-plugin).

---

### Installing the shell plugin

Once the binary is installed, the shell plugin requires installing.
For zsh, bash, fish, and PowerShell, `atuin setup` installs it for you.
The install script runs `atuin setup`, so if you used the install script, this is already done.
After installing, remember to restart your shell.

To hook only some shells and change nothing else, pass `--only` and name the shells with
`--shells`. For example, this command hooks zsh and fish:

```shell
atuin setup --only --shell-history --shells zsh,fish
```

=== "zsh"

    Run setup, which adds `eval "$(atuin init zsh)"` to `$ZDOTDIR/.zshrc`, or to `~/.zshrc` when
    `ZDOTDIR` isn't set:

    ```shell
    atuin setup --only --shell-history --shells zsh
    ```

    If you use a plugin manager, load Atuin with it instead.

    === "zinit"

        ```shell
        # if you _only_ want to install the shell-plugin, do this; otherwise look above for a "everything via zinit" solution
        zinit load atuinsh/atuin
        ```

    === "Antigen"

        ```shell
        antigen bundle atuinsh/atuin@main
        ```

    === "Antidote"

        ```shell
        antidote install atuinsh/atuin
        ```

=== "bash"

    === "ble.sh"

        Atuin works best in bash when using [ble.sh](https://github.com/akinomyoga/ble.sh) >= 0.4.

        With ble.sh (>= 0.4) installed and loaded in `~/.bashrc`, run setup, which adds
        `eval "$(atuin init bash)"` to the end of your `~/.bashrc`:

        ```shell
        atuin setup --only --shell-history --shells bash
        ```

    === "bash-preexec"

        [bash-preexec](https://github.com/rcaloras/bash-preexec) can also be used, but you may experience
         some minor problems with the recorded duration and exit status of some commands.

        !!! warning "Please note"

            bash-preexec currently has [an issue][bp-ignorespace] where it will stop
            honoring `ignorespace`. While Atuin will ignore commands prefixed with
            whitespace, they may still end up in your bash history. Please check your
            configuration! All other shells don't have this issue.

            To use `atuin < 18.10.0` in `bash < 4` with bash-preexec, the option
            `enter_accept` needs to be turned on (which is so by default).  There is no
            restriction in the latest version of Atuin (>= 18.10.0).

            bash-preexec can't properly invoke the `preexec` hook for subshell commands
            `(...)`, function definitions `func() { ...; }`, empty for-in-statements `for
            i in; do ...; done`, etc., so those commands and duration may not be recorded
            in the Atuin's history correctly.

        As of Atuin 18.18.0, `atuin init bash` will automatically load bash-preexec if no other
        preexec backend has been loaded (ble.sh or an external copy of bash-preexec). To use the
        built-in copy, run setup, which adds `eval "$(atuin init bash)"` to your `~/.bashrc`:

        ```shell
        atuin setup --only --shell-history --shells bash
        ```

        To disable the built-in copy, pass `ATUIN_NO_BUILTIN_PREEXEC=1` to `atuin init` in your
        `~/.bashrc`, e.g.:

        ```shell
        eval "$(ATUIN_NO_BUILTIN_PREEXEC=1 atuin init bash)"
        ```

        If you prefer, you can also download and install bash-preexec separately:

        ```shell
        curl https://raw.githubusercontent.com/rcaloras/bash-preexec/master/bash-preexec.sh -o ~/.bash-preexec.sh
        echo '[[ -f ~/.bash-preexec.sh ]] && source ~/.bash-preexec.sh' >> ~/.bashrc
        ```

        Then run setup, which adds Atuin to the end of your `~/.bashrc`, after bash-preexec:

        ```shell
        atuin setup --only --shell-history --shells bash
        ```

=== "fish"

    Run setup, which adds an `is-interactive` block that runs `atuin init fish | source` to your
    `~/.config/fish/config.fish` file:

    ```shell
    atuin setup --only --shell-history --shells fish
    ```

=== "Nushell"

    Run in *Nushell*:

    ```shell
    mkdir ~/.atuin/
    atuin init nu | save ~/.atuin/init.nu
    ```

    Add to `config.nu`:

    ```shell
    source ~/.atuin/init.nu
    ```

    ??? tip "Optional: Atuin pty-proxy"
        pty-proxy is a lightweight pty proxy that renders the Atuin popup over
        your previous output, restoring it when closed — no clearing, no
        fullscreen. To use pty-proxy with Nushell, generate the init script:

        ```shell
        mkdir ~/.atuin/
        atuin pty-proxy init nu | save -f ~/.atuin/pty-proxy-init.nu
        ```

        Then source it as early as possible in your `config.nu`, *before*
        the regular atuin init:

        ```shell
        source ~/.atuin/pty-proxy-init.nu
        source ~/.atuin/init.nu
        ```

        Nushell's `source` command requires a static file path, so you must
        pre-generate both files.

=== "xonsh"

    Add
    ```shell
    execx($(atuin init xonsh))
    ```
    to the end of your `~/.xonshrc`

=== "PowerShell"

    Run setup, which adds `atuin init powershell | Out-String | Invoke-Expression` to the end of
    your PowerShell profile:

    ```shell
    atuin setup --only --shell-history --shells powershell
    ```

    On Windows, setup writes to the profiles of both PowerShell 7 and Windows PowerShell, in your
    Documents folder. Elsewhere, setup writes to
    `~/.config/powershell/Microsoft.PowerShell_profile.ps1`.

## Upgrade

Run `atuin update`, and if that command isn't available, run the install script again.

If GitHub's API rate limit prevents the update, set `ATUIN_GITHUB_TOKEN` to a GitHub token. For example, if you use the GitHub CLI:

```shell
ATUIN_GITHUB_TOKEN="$(gh auth token)" atuin update
```

If you used a package manager to install Atuin, then you should also use your package manager to update Atuin.

## Uninstall

If you'd like to uninstall Atuin, please check out [the uninstall page](../uninstall.md).

[bp-ignorespace]: https://github.com/rcaloras/bash-preexec/issues/115
