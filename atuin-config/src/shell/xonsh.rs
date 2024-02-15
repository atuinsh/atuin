use super::Alias;

// Configuration for xonsh
pub fn build(aliases: &[Alias]) -> String {
    let mut config = String::new();

    for alias in aliases {
        config.push_str(&format!("aliases['{}'] ='{}'\n", alias.name, alias.value));
    }

    config
}
