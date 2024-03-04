use super::Alias;

// Configuration for bash
pub fn build(aliases: &[Alias]) -> String {
    let mut config = String::new();

    for alias in aliases {
        config.push_str(&format!("alias {}='{}'\n", alias.name, alias.value));
    }

    config
}
