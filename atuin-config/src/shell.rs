pub mod bash;
pub mod fish;
pub mod xonsh;
pub mod zsh;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    pub name: String,
    pub value: String,
}
