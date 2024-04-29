pub async fn var_config() -> String {
    // Because nushell won't autoupdate, we just parse the output of `atuin dotfiles var list` in
    // nushell and load the env vars that way

    String::from(
        r#"atuin dotfiles var list | lines | parse "export {name}={value}" | reduce -f {} {|it, acc| $acc | upsert $it.name $it.value} | load-env"#,
    )
}
