<h1 align="center">
  A'tuin
</h1>
<blockquote align="center">
  Through the fathomless deeps of space swims the star turtle Great A’Tuin, bearing on its back the four giant elephants who carry on their shoulders the mass of the Discworld.
 </blockquote>
 
`atuin` manages and synchronizes your shell history! Instead of storing 
everything in a text file (such as ~/.history), `atuin` uses a sqlite database.
This lets us do all kinds of analysis on it!

As well as the expected command, this stores

- duration
- exit code
- working directory
- hostname
- time
- a unique session ID

## Install

`atuin` needs a recent version of Rust + Cargo! It's best to use rustup for.

```
cargo install atuin
```

and then add this to your ~/.zshrc

```
export ATUIN_SESSION=$(atuin uuid)

_atuin_preexec(){
	id=$(atuin history start $1)
	export ATUIN_HISTORY_ID="$id"
}

_atuin_precmd(){
	local EXIT="$?"

	[[ -z "${ATUIN_HISTORY_ID}" ]] && return

	atuin history end $ATUIN_HISTORY_ID --exit $EXIT
}

add-zsh-hook preexec _atuin_preexec
add-zsh-hook precmd _atuin_precmd
```

We're not replacing anything here, so your default shell history file will still
be written to!

## Usage

### Import history

```
atuin import auto # detect shell, then import
atuin import zsh  # specify shell
```

### List history

```
atuin history list
```
