# Source this in your ~/.zshrc

_atuin_preexec(){
    atuin history add $1
}

add-zsh-hook preexec _atuin_preexec
