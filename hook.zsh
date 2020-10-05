# Source this in your ~/.zshrc

_shync_preexec(){
    shync history add $1
}

add-zsh-hook preexec _shync_preexec
