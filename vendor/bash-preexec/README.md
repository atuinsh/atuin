[![Build Status](https://github.com/rcaloras/bash-preexec/actions/workflows/bats.yaml/badge.svg)](https://github.com/rcaloras/bash-preexec/actions/)
[![GitHub version](https://badge.fury.io/gh/rcaloras%2Fbash-preexec.svg)](https://badge.fury.io/gh/rcaloras%2Fbash-preexec)

Bash-Preexec 
============

**preexec** and **precmd** hook functions for Bash 3.1+ in the style of Zsh. They aim to emulate the behavior [as described for Zsh](http://zsh.sourceforge.net/Doc/Release/Functions.html#Hook-Functions).

<a href="https://bashhub.com" target="_blank"><img src="https://bashhub.com/static/web/images/bashhub-logo.png" alt="Bashhub Logo" width="200"></a>

This project is currently being used in production by [Bashhub](https://github.com/rcaloras/bashhub-client), [iTerm2](https://github.com/gnachman/iTerm2), and [Ghostty](https://ghostty.org/). Hype!

## Quick Start
```bash
# Pull down our file from GitHub and write it to your home directory as a hidden file.
curl https://raw.githubusercontent.com/rcaloras/bash-preexec/master/bash-preexec.sh -o ~/.bash-preexec.sh
# Source our file to bring it into our environment
source ~/.bash-preexec.sh
# Define a couple functions.
preexec() { echo "just typed $1"; }
precmd() { echo "printing the prompt"; }
```

## Install
You'll want to pull down the file and add it to your bash profile/configuration (i.e ~/.bashrc, ~/.profile, ~/.bash_profile, etc). **It must be the last thing imported in your bash profile.**
```bash
# Pull down our file from GitHub and write it to your home directory as a hidden file.
curl https://raw.githubusercontent.com/rcaloras/bash-preexec/master/bash-preexec.sh -o ~/.bash-preexec.sh
# Source our file at the end of our bash profile (e.g. ~/.bashrc, ~/.profile, or ~/.bash_profile)
echo '[[ -f ~/.bash-preexec.sh ]] && source ~/.bash-preexec.sh' >> ~/.bashrc
```

## Usage
Two functions **preexec** and **precmd** can now be defined and they'll be automatically invoked by bash-preexec if they exist.

* `preexec` Executed just after a command has been read and is about to be executed. The string that the user typed is passed as the first argument.
* `precmd` Executed just before each prompt. Equivalent to PROMPT_COMMAND, but more flexible and resilient.
```bash
source ~/.bash-preexec.sh
preexec() { echo "just typed $1"; }
precmd() { echo "printing the prompt"; }
```
Should output something like:
```
elementz@Kashmir:~/git/bash-preexec (master)$ ls
just typed ls
bash-preexec.sh  README.md  test
printing the prompt
```
#### Function Arrays
You can also define functions to be invoked by appending them to two different arrays. This is great if you want to have many functions invoked for either hook. Both preexec and precmd functions are added to these by default and don't need to be added manually.
* `$preexec_functions` Array of functions invoked by preexec.
* `$precmd_functions` Array of functions invoked by precmd.

#### preexec
```bash
# Define some function to use preexec
preexec_hello_world() { echo "You just entered $1"; }
# Add it to the array of functions to be invoked each time.
preexec_functions+=(preexec_hello_world)
```

#### precmd
```bash
precmd_hello_world() { echo "This is invoked before the prompt is displayed"; }
precmd_functions+=(precmd_hello_world)
```

You can also define multiple functions to be invoked like so.

```bash
precmd_hello_one() { echo "This is invoked on precmd first"; }
precmd_hello_two() { echo "This is invoked on precmd second"; }
precmd_functions+=(precmd_hello_one)
precmd_functions+=(precmd_hello_two)
```

You can check the functions set for each by echoing its contents.

```bash
echo ${preexec_functions[@]}
echo ${precmd_functions[@]}
```

## Subshells
bash-preexec does not support invoking preexec() for subshells by default. It must be enabled by setting 
`__bp_enable_subshells`.
```bash
# Enable experimental subshell support
export __bp_enable_subshells="true"
```
This is disabled by default due to buggy situations related to to `functrace` and Bash's `DEBUG trap`. See [Issue #25](https://github.com/rcaloras/bash-preexec/issues/25)

## Library authors
If you want to detect bash-preexec in your library (for example, to add hooks to `preexec_functions` when available), use the Bash variable `bash_preexec_imported`:

```bash
if [[ -n "${bash_preexec_imported:-}" ]]; then
    echo "Bash-preexec is loaded."
fi
```

## Tests
You can run tests using [Bats](https://github.com/bats-core/bats-core).
```bash
bats test
```
Should output something like:
```
elementz@Kashmir:~/git/bash-preexec(master)$ bats test
 ✓ No functions defined for preexec should simply return
 ✓ precmd should execute a function once
 ✓ preexec should execute a function with the last command in our history
 ✓ preexec should execute multiple functions in the order added to their arrays
 ✓ preecmd should execute multiple functions in the order added to their arrays
```
